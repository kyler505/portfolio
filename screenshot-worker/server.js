const dns = require("node:dns").promises;
const http = require("node:http");
const net = require("node:net");
const { URL } = require("node:url");
const { chromium } = require("playwright");

const DEFAULT_PORT = 3001;
const DEFAULT_CAPTURE_TIMEOUT_MS = 8000;
const DEFAULT_DNS_LOOKUP_TIMEOUT_MS = 2000;
const DEFAULT_VIEWPORT = Object.freeze({ width: 1366, height: 768 });
const HEALTH_CHECK_PAYLOAD = Object.freeze({ ok: true, status: "up" });

const port = readBoundedInt("PORT", DEFAULT_PORT, 1, 65535);
const captureTimeoutMs = readBoundedInt("CAPTURE_TIMEOUT_MS", DEFAULT_CAPTURE_TIMEOUT_MS, 1000, 120000);
const dnsLookupTimeoutMs = readBoundedInt("DNS_LOOKUP_TIMEOUT_MS", DEFAULT_DNS_LOOKUP_TIMEOUT_MS, 100, 30000);
const workerToken = readOptionalString("SCREENSHOT_WORKER_TOKEN");

let browserPromise;

function readOptionalString(name) {
  const value = process.env[name];
  if (typeof value !== "string") {
    return null;
  }

  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

function readBoundedInt(name, fallback, min, max) {
  const raw = readOptionalString(name);
  if (!raw) {
    return fallback;
  }

  const parsed = Number.parseInt(raw, 10);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) {
    return fallback;
  }

  return parsed;
}

function jsonResponse(res, statusCode, payload) {
  const body = JSON.stringify(payload);
  res.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  res.end(body);
}

function handleHealthCheck(req, res, pathname) {
  if (pathname !== "/health" && pathname !== "/uptime") {
    return false;
  }

  if (req.method === "GET") {
    jsonResponse(res, 200, HEALTH_CHECK_PAYLOAD);
    return true;
  }

  if (req.method === "HEAD") {
    res.writeHead(200, {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
    });
    res.end();
    return true;
  }

  jsonResponse(res, 404, { ok: false, error: "not found" });
  return true;
}

function parseIpv4(address) {
  const parts = address.split(".");
  if (parts.length !== 4) {
    return null;
  }

  const octets = parts.map((part) => Number.parseInt(part, 10));
  if (octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)) {
    return null;
  }

  return octets;
}

function isBlockedIpv4(address) {
  const octets = parseIpv4(address);
  if (!octets) {
    return true;
  }

  const [first, second, third] = octets;
  if (first === 10 || first === 127 || first === 0) {
    return true;
  }

  if (first === 169 && second === 254) {
    return true;
  }

  if (first === 172 && second >= 16 && second <= 31) {
    return true;
  }

  if (first === 192 && second === 168) {
    return true;
  }

  if (first === 100 && second >= 64 && second <= 127) {
    return true;
  }

  if (first === 192 && second === 0 && third === 2) {
    return true;
  }

  if (first === 198 && second === 51 && third === 100) {
    return true;
  }

  if (first === 203 && second === 0 && third === 113) {
    return true;
  }

  if (first >= 224) {
    return true;
  }

  return false;
}

function parseIpv4MappedIpv6(address) {
  const normalized = address.trim().toLowerCase();
  if (!normalized.includes("::ffff:")) {
    return null;
  }

  const suffix = normalized.split("::ffff:").pop();
  if (!suffix) {
    return null;
  }

  return parseIpv4(suffix) ? suffix : null;
}

function isBlockedIpv6(address) {
  const normalized = address.trim().toLowerCase();
  if (normalized === "::" || normalized === "::1") {
    return true;
  }

  const mappedIpv4 = parseIpv4MappedIpv6(normalized);
  if (mappedIpv4) {
    return isBlockedIpv4(mappedIpv4);
  }

  if (normalized.startsWith("fc") || normalized.startsWith("fd")) {
    return true;
  }

  if (normalized.startsWith("fe8") || normalized.startsWith("fe9") || normalized.startsWith("fea") || normalized.startsWith("feb")) {
    return true;
  }

  if (normalized.startsWith("ff")) {
    return true;
  }

  return normalized.startsWith("2001:db8");
}

function isBlockedIp(address) {
  const type = net.isIP(address);
  if (type === 4) {
    return isBlockedIpv4(address);
  }

  if (type === 6) {
    return isBlockedIpv6(address);
  }

  return true;
}

function readBearerToken(req) {
  const authorization = req.headers.authorization;
  if (typeof authorization !== "string") {
    return null;
  }

  const prefix = "Bearer ";
  if (!authorization.startsWith(prefix)) {
    return null;
  }

  return authorization.slice(prefix.length).trim();
}

async function validateTargetUrl(rawUrl) {
  return validateTargetUrlWithPolicy(rawUrl, null);
}

function withLookupTimeout(host) {
  return new Promise((resolve, reject) => {
    const timeoutId = setTimeout(() => {
      const timeoutError = new Error(`dns lookup timed out for ${host}`);
      timeoutError.code = "DNS_LOOKUP_TIMEOUT";
      reject(timeoutError);
    }, dnsLookupTimeoutMs);

    dns
      .lookup(host, { all: true, verbatim: true })
      .then((records) => {
        clearTimeout(timeoutId);
        resolve(records);
      })
      .catch((error) => {
        clearTimeout(timeoutId);
        reject(error);
      });
  });
}

async function validateTargetUrlWithPolicy(rawUrl, requiredMainFrameHost) {
  let parsed;
  try {
    parsed = new URL(rawUrl);
  } catch {
    return { error: "invalid URL" };
  }

  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return { error: "URL scheme must be http or https" };
  }

  const host = parsed.hostname.toLowerCase();
  if (host === "localhost" || host.endsWith(".localhost")) {
    return { error: "local addresses are not allowed" };
  }

  if (requiredMainFrameHost && host !== requiredMainFrameHost) {
    return { error: "main-frame redirects must remain on the original host" };
  }

  if (net.isIP(host)) {
    if (isBlockedIp(host)) {
      return { error: "host address is blocked" };
    }

    return { value: parsed };
  }

  let resolved;
  try {
    resolved = await withLookupTimeout(host);
  } catch (error) {
    if (error && error.code === "DNS_LOOKUP_TIMEOUT") {
      return { error: "DNS lookup timed out" };
    }

    return { error: "unable to resolve host" };
  }

  if (!Array.isArray(resolved) || resolved.length === 0) {
    return { error: "unable to resolve host" };
  }

  for (const record of resolved) {
    if (!record || typeof record.address !== "string" || isBlockedIp(record.address)) {
      return { error: "host address is blocked" };
    }
  }

  return { value: parsed };
}

async function validateRequestUrl(rawUrl, requiredMainFrameHost) {
  try {
    const validation = await validateTargetUrlWithPolicy(rawUrl, requiredMainFrameHost);
    return validation;
  } catch {
    return { error: "unable to validate request URL" };
  }
}

async function abortRoute(route) {
  try {
    await route.abort("blockedbyclient");
  } catch {
    // Ignore already-handled route failures.
  }
}

function isMainFrameDocumentNavigation(request) {
  if (!request.isNavigationRequest() || request.resourceType() !== "document") {
    return false;
  }

  const frame = request.frame();
  return frame.parentFrame() === null;
}

async function getBrowser() {
  if (!browserPromise) {
    browserPromise = chromium.launch({
      headless: true,
      args: ["--disable-dev-shm-usage", "--no-sandbox"],
    });
  }

  return browserPromise;
}

async function captureScreenshotAsDataUrl(targetUrl) {
  const browser = await getBrowser();
  const context = await browser.newContext({ viewport: DEFAULT_VIEWPORT });
  const mainFrameHost = targetUrl.hostname.toLowerCase();

  try {
    const page = await context.newPage();
    await page.route("**/*", async (route) => {
      try {
        const request = route.request();
        const requiredMainFrameHost = isMainFrameDocumentNavigation(request) ? mainFrameHost : null;
        const validation = await validateRequestUrl(request.url(), requiredMainFrameHost);
        if (validation.error) {
          await abortRoute(route);
          return;
        }

        await route.continue();
      } catch {
        await abortRoute(route);
      }
    });

    page.setDefaultNavigationTimeout(captureTimeoutMs);
    await page.goto(targetUrl.toString(), {
      timeout: captureTimeoutMs,
      waitUntil: "networkidle",
    });
    const screenshot = await page.screenshot({
      type: "png",
      fullPage: false,
      timeout: captureTimeoutMs,
    });

    return `data:image/png;base64,${Buffer.from(screenshot).toString("base64")}`;
  } finally {
    await context.close();
  }
}

const server = http.createServer(async (req, res) => {
  if (!req.url) {
    jsonResponse(res, 400, { ok: false, error: "missing request URL" });
    return;
  }

  const requestUrl = new URL(req.url, `http://127.0.0.1:${port}`);

  if (handleHealthCheck(req, res, requestUrl.pathname)) {
    return;
  }

  if (requestUrl.pathname !== "/capture" || req.method !== "GET") {
    jsonResponse(res, 404, { ok: false, error: "not found" });
    return;
  }

  if (workerToken) {
    const providedToken = readBearerToken(req);
    if (!providedToken || providedToken !== workerToken) {
      jsonResponse(res, 401, { ok: false, error: "unauthorized" });
      return;
    }
  }

  const rawTargetUrl = requestUrl.searchParams.get("url");
  if (!rawTargetUrl) {
    jsonResponse(res, 400, { ok: false, error: "missing url query parameter" });
    return;
  }

  const validation = await validateTargetUrl(rawTargetUrl);
  if (validation.error) {
    jsonResponse(res, 400, { ok: false, error: validation.error });
    return;
  }

  try {
    const image = await captureScreenshotAsDataUrl(validation.value);
    jsonResponse(res, 200, { ok: true, image });
  } catch {
    jsonResponse(res, 502, { ok: false, error: "failed to capture screenshot" });
  }
});

server.listen(port, "0.0.0.0", () => {
  console.log(`screenshot worker listening on http://0.0.0.0:${port}`);
});

async function shutdown(code) {
  server.close();

  if (browserPromise) {
    try {
      const browser = await browserPromise;
      await browser.close();
    } catch {
      // Ignore browser shutdown failures.
    }
  }

  process.exit(code);
}

process.on("SIGTERM", () => {
  shutdown(0);
});

process.on("SIGINT", () => {
  shutdown(0);
});
