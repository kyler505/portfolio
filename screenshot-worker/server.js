const dns = require("node:dns").promises;
const http = require("node:http");
const { randomUUID } = require("node:crypto");
const net = require("node:net");
const { URL } = require("node:url");
const { chromium } = require("playwright");

const DEFAULT_PORT = 3001;
const DEFAULT_CAPTURE_TIMEOUT_MS = 8000;
const DEFAULT_NAVIGATION_TIMEOUT_MS = 7000;
const DEFAULT_SCREENSHOT_TIMEOUT_MS = 12000;
const DEFAULT_SETTLE_WAIT_MS = 700;
const DEFAULT_DNS_LOOKUP_TIMEOUT_MS = 2000;
const DEFAULT_VIEWPORT = Object.freeze({ width: 1366, height: 768 });
const DEFAULT_BROWSER_LOCALE = "en-US";
const DEFAULT_BROWSER_TIMEZONE = "America/New_York";
const DEFAULT_ACCEPT_LANGUAGE = "en-US,en;q=0.9";
const DEFAULT_DESKTOP_USER_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36";
const COMMON_SECOND_LEVEL_SUFFIXES = new Set(["ac", "co", "com", "edu", "gov", "net", "org"]);
const HEALTH_CHECK_PAYLOAD = Object.freeze({ ok: true, status: "up" });

const port = readBoundedInt("PORT", DEFAULT_PORT, 1, 65535);
const captureTimeoutMs = readBoundedInt("CAPTURE_TIMEOUT_MS", DEFAULT_CAPTURE_TIMEOUT_MS, 1000, 120000);
const navigationTimeoutMs = readBoundedInt("CAPTURE_NAV_TIMEOUT_MS", DEFAULT_NAVIGATION_TIMEOUT_MS, 500, 120000);
const screenshotTimeoutMs = readBoundedInt(
  "CAPTURE_SCREENSHOT_TIMEOUT_MS",
  DEFAULT_SCREENSHOT_TIMEOUT_MS,
  500,
  120000,
);
const settleWaitMs = readBoundedInt("CAPTURE_SETTLE_WAIT_MS", DEFAULT_SETTLE_WAIT_MS, 0, 15000);
const dnsLookupTimeoutMs = readBoundedInt("DNS_LOOKUP_TIMEOUT_MS", DEFAULT_DNS_LOOKUP_TIMEOUT_MS, 100, 30000);
const workerToken = readOptionalString("SCREENSHOT_WORKER_TOKEN");
const logLevel = readLogLevel("LOG_LEVEL", "info");
const logPreviewUrlMode = readUrlLogMode("LOG_PREVIEW_URL_MODE", "host");

let browserPromise;

function readLogLevel(name, fallback) {
  const value = readOptionalString(name);
  if (!value) {
    return fallback;
  }

  const normalized = value.toLowerCase();
  if (normalized === "debug" || normalized === "info") {
    return normalized;
  }

  return fallback;
}

function readUrlLogMode(name, fallback) {
  const value = readOptionalString(name);
  if (!value) {
    return fallback;
  }

  const normalized = value.toLowerCase();
  if (normalized === "host" || normalized === "full") {
    return normalized;
  }

  return fallback;
}

function shouldLog(level) {
  if (logLevel === "debug") {
    return true;
  }

  return level !== "debug";
}

function logEvent(level, event, fields = {}) {
  if (!shouldLog(level)) {
    return;
  }

  const payload = {
    ts: Math.floor(Date.now() / 1000),
    level,
    event,
    ...fields,
  };

  process.stdout.write(`${JSON.stringify(payload)}\n`);
}

function createRequestId() {
  return `req-${Date.now()}-${randomUUID()}`;
}

function readRequestId(req) {
  const value = req.headers["x-request-id"];
  if (typeof value !== "string") {
    return createRequestId();
  }

  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : createRequestId();
}

function sanitizeUrlForLogs(value) {
  try {
    const parsed = new URL(value);
    if (logPreviewUrlMode === "full") {
      return parsed.toString();
    }

    if (!parsed.host) {
      return "unknown";
    }

    return parsed.host;
  } catch {
    return "invalid";
  }
}

function classifyCaptureError(error) {
  if (!error || typeof error !== "object") {
    return { errorClass: "unknown_error" };
  }

  const errorName = typeof error.name === "string" ? error.name : "";
  const errorCode = typeof error.code === "string" ? error.code : "";
  const rawMessage = typeof error.message === "string" ? error.message : "";
  const message = rawMessage.toLowerCase();
  const normalizedCode = errorCode.toLowerCase();
  const normalizedName = errorName.toLowerCase();

  if (
    normalizedCode.includes("timeout") ||
    normalizedName.includes("timeout") ||
    message.includes("timeout")
  ) {
    return {
      errorClass: "playwright_timeout",
      reason: sanitizeErrorReason(errorCode || errorName || "timeout"),
    };
  }

  if (
    message.includes("navigation") ||
    message.includes("net::") ||
    message.includes("page.goto")
  ) {
    return {
      errorClass: "navigation_failed",
      reason: sanitizeErrorReason(errorCode || errorName || "navigation"),
    };
  }

  if (message.includes("screenshot") || message.includes("page.screenshot")) {
    return {
      errorClass: "screenshot_failed",
      reason: sanitizeErrorReason(errorCode || errorName || "screenshot"),
    };
  }

  return {
    errorClass: "unknown_error",
    reason: sanitizeErrorReason(errorCode || errorName),
  };
}

function sanitizeErrorReason(value) {
  if (typeof value !== "string") {
    return null;
  }

  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "_")
    .replace(/^_+|_+$/g, "");

  if (normalized.length === 0) {
    return null;
  }

  return normalized.slice(0, 64);
}

function createCaptureTimeoutError(stage) {
  const error = new Error(`capture timeout at ${stage}`);
  error.code = "CAPTURE_TIMEOUT";
  error.captureStage = stage;
  return error;
}

function getRemainingBudget(deadlineMs) {
  return deadlineMs - Date.now();
}

function reserveStageTimeout(configuredTimeoutMs, deadlineMs, stage) {
  const remainingBudgetMs = getRemainingBudget(deadlineMs);
  if (remainingBudgetMs <= 0) {
    throw createCaptureTimeoutError(stage);
  }

  return Math.min(configuredTimeoutMs, remainingBudgetMs);
}

function waitFor(ms) {
  if (ms <= 0) {
    return Promise.resolve();
  }

  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function logCaptureStageTiming(requestId, targetUrl, stage, stageStartedAt, captureStartedAt, extra = {}) {
  logEvent("info", "capture_stage_timing", {
    request_id: requestId,
    target_url: sanitizeUrlForLogs(targetUrl.toString()),
    stage,
    stage_duration_ms: Date.now() - stageStartedAt,
    elapsed_ms: Date.now() - captureStartedAt,
    ...extra,
  });
}

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

function completeRequestLog(context, statusCode) {
  if (!context || context.completed) {
    return;
  }

  context.completed = true;
  logEvent("info", "worker_request_end", {
    request_id: context.requestId,
    method: context.method,
    path: context.path,
    status: statusCode,
    duration_ms: Date.now() - context.startedAt,
  });
}

function jsonResponse(res, statusCode, payload, context) {
  const body = JSON.stringify(payload);
  const headers = {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  };
  if (context) {
    headers["x-request-id"] = context.requestId;
  }
  res.writeHead(statusCode, headers);
  res.end(body);
  completeRequestLog(context, statusCode);
}

function headResponse(res, statusCode, context) {
  const headers = {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  };
  if (context) {
    headers["x-request-id"] = context.requestId;
  }
  res.writeHead(statusCode, headers);
  res.end();
  completeRequestLog(context, statusCode);
}

function handleHealthCheck(req, res, pathname, context) {
  if (pathname !== "/" && pathname !== "/health" && pathname !== "/uptime") {
    return false;
  }

  if (req.method === "GET") {
    jsonResponse(res, 200, HEALTH_CHECK_PAYLOAD, context);
    return true;
  }

  if (req.method === "HEAD") {
    headResponse(res, 200, context);
    return true;
  }

  jsonResponse(res, 404, { ok: false, error: "not found" }, context);
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

function isSubdomainOrExactHost(host, candidateBase) {
  return host === candidateBase || host.endsWith(`.${candidateBase}`);
}

function getRegistrableDomain(host) {
  if (net.isIP(host)) {
    return null;
  }

  const labels = host.toLowerCase().split(".").filter((label) => label.length > 0);
  if (labels.length < 2) {
    return null;
  }

  const topLevelLabel = labels[labels.length - 1];
  const secondLevelLabel = labels[labels.length - 2];
  const shouldUseThreeLabels =
    topLevelLabel.length === 2 && COMMON_SECOND_LEVEL_SUFFIXES.has(secondLevelLabel) && labels.length >= 3;

  const suffixLength = shouldUseThreeLabels ? 3 : 2;
  return labels.slice(-suffixLength).join(".");
}

function isAllowedMainFrameRedirectHost(host, requiredMainFrameHost) {
  if (isSubdomainOrExactHost(host, requiredMainFrameHost) || isSubdomainOrExactHost(requiredMainFrameHost, host)) {
    return true;
  }

  const hostRegistrableDomain = getRegistrableDomain(host);
  const requiredRegistrableDomain = getRegistrableDomain(requiredMainFrameHost);
  if (!hostRegistrableDomain || !requiredRegistrableDomain) {
    return false;
  }

  return hostRegistrableDomain === requiredRegistrableDomain;
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
    return { error: "invalid URL", reason: "invalid_url" };
  }

  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return { error: "URL scheme must be http or https", reason: "invalid_scheme" };
  }

  const host = parsed.hostname.toLowerCase();
  if (host === "localhost" || host.endsWith(".localhost")) {
    return { error: "local addresses are not allowed", reason: "blocked_ip" };
  }

  if (requiredMainFrameHost && !isAllowedMainFrameRedirectHost(host, requiredMainFrameHost)) {
    return { error: "main-frame redirects must remain on the original domain", reason: "redirect_host_block" };
  }

  if (net.isIP(host)) {
    if (isBlockedIp(host)) {
      return { error: "host address is blocked", reason: "blocked_ip" };
    }

    return { value: parsed };
  }

  let resolved;
  try {
    resolved = await withLookupTimeout(host);
  } catch (error) {
    if (error && error.code === "DNS_LOOKUP_TIMEOUT") {
      return { error: "DNS lookup timed out", reason: "dns_timeout" };
    }

    return { error: "unable to resolve host", reason: "dns_resolve_failed" };
  }

  if (!Array.isArray(resolved) || resolved.length === 0) {
    return { error: "unable to resolve host", reason: "dns_resolve_failed" };
  }

  for (const record of resolved) {
    if (!record || typeof record.address !== "string" || isBlockedIp(record.address)) {
      return { error: "host address is blocked", reason: "blocked_ip" };
    }
  }

  return { value: parsed };
}

async function validateRequestUrl(rawUrl, requiredMainFrameHost) {
  try {
    const validation = await validateTargetUrlWithPolicy(rawUrl, requiredMainFrameHost);
    return validation;
  } catch {
    return { error: "unable to validate request URL", reason: "validation_error" };
  }
}

async function abortRoute(route, reason, requestId) {
  logEvent("debug", "capture_route_abort", {
    request_id: requestId,
    reason,
    url: sanitizeUrlForLogs(route.request().url()),
  });

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

async function captureScreenshotAsDataUrl(targetUrl, requestId) {
  const browser = await getBrowser();
  const context = await browser.newContext({
    viewport: DEFAULT_VIEWPORT,
    locale: DEFAULT_BROWSER_LOCALE,
    timezoneId: DEFAULT_BROWSER_TIMEZONE,
    userAgent: DEFAULT_DESKTOP_USER_AGENT,
    extraHTTPHeaders: {
      "Accept-Language": DEFAULT_ACCEPT_LANGUAGE,
    },
  });
  const mainFrameHost = targetUrl.hostname.toLowerCase();
  const captureStartedAt = Date.now();
  const captureDeadlineMs = captureStartedAt + captureTimeoutMs;

  try {
    const page = await context.newPage();
    await page.route("**/*", async (route) => {
      try {
        const request = route.request();
        const requiredMainFrameHost = isMainFrameDocumentNavigation(request) ? mainFrameHost : null;
        const validation = await validateRequestUrl(request.url(), requiredMainFrameHost);
        if (validation.error) {
          await abortRoute(route, validation.reason || "validation_failed", requestId);
          return;
        }

        await route.continue();
      } catch {
        await abortRoute(route, "route_handler_failed", requestId);
      }
    });

    page.setDefaultNavigationTimeout(Math.min(navigationTimeoutMs, captureTimeoutMs));
    page.setDefaultTimeout(Math.min(screenshotTimeoutMs, captureTimeoutMs));
    logEvent("info", "capture_goto_start", {
      request_id: requestId,
      url: sanitizeUrlForLogs(targetUrl.toString()),
      wait_until: "domcontentloaded",
      navigation_timeout_ms: Math.min(navigationTimeoutMs, captureTimeoutMs),
      total_timeout_ms: captureTimeoutMs,
    });
    const gotoStartedAt = Date.now();
    const gotoTimeoutMs = reserveStageTimeout(navigationTimeoutMs, captureDeadlineMs, "goto");
    await page.goto(targetUrl.toString(), {
      timeout: gotoTimeoutMs,
      waitUntil: "domcontentloaded",
    });
    logEvent("info", "capture_goto_ok", {
      request_id: requestId,
      url: sanitizeUrlForLogs(targetUrl.toString()),
    });
    logCaptureStageTiming(requestId, targetUrl, "goto", gotoStartedAt, captureStartedAt, {
      timeout_ms: gotoTimeoutMs,
    });

    const settleStartedAt = Date.now();
    const settleBudgetMs = reserveStageTimeout(settleWaitMs, captureDeadlineMs, "settle_wait");
    await waitFor(settleBudgetMs);
    logCaptureStageTiming(requestId, targetUrl, "settle_wait", settleStartedAt, captureStartedAt, {
      settle_wait_ms: settleBudgetMs,
    });

    const screenshotStartedAt = Date.now();
    const screenshotBudgetMs = reserveStageTimeout(screenshotTimeoutMs, captureDeadlineMs, "screenshot");
    logEvent("info", "capture_screenshot_start", {
      request_id: requestId,
      url: sanitizeUrlForLogs(targetUrl.toString()),
      screenshot_timeout_ms: screenshotBudgetMs,
    });
    const screenshot = await page.screenshot({
      type: "png",
      fullPage: false,
      timeout: screenshotBudgetMs,
    });
    logEvent("info", "capture_screenshot_ok", {
      request_id: requestId,
      url: sanitizeUrlForLogs(targetUrl.toString()),
    });
    logCaptureStageTiming(requestId, targetUrl, "screenshot", screenshotStartedAt, captureStartedAt, {
      screenshot_timeout_ms: screenshotBudgetMs,
      bytes: screenshot.byteLength,
    });

    return `data:image/png;base64,${Buffer.from(screenshot).toString("base64")}`;
  } catch (error) {
    logEvent("info", "capture_stage_failed", {
      request_id: requestId,
      target_url: sanitizeUrlForLogs(targetUrl.toString()),
      stage: error && typeof error.captureStage === "string" ? error.captureStage : "unknown",
      elapsed_ms: Date.now() - captureStartedAt,
    });
    throw error;
  } finally {
    await context.close();
  }
}

const server = http.createServer(async (req, res) => {
  const requestId = readRequestId(req);
  const context = {
    requestId,
    method: req.method || "UNKNOWN",
    path: "unknown",
    startedAt: Date.now(),
    completed: false,
  };

  if (!req.url) {
    context.path = "unknown";
    logEvent("info", "worker_request_start", {
      request_id: requestId,
      method: context.method,
      path: context.path,
    });
    jsonResponse(res, 400, { ok: false, error: "missing request URL" }, context);
    return;
  }

  const requestUrl = new URL(req.url, `http://127.0.0.1:${port}`);
  context.path = requestUrl.pathname;

  logEvent("info", "worker_request_start", {
    request_id: requestId,
    method: context.method,
    path: context.path,
  });

  if (handleHealthCheck(req, res, requestUrl.pathname, context)) {
    return;
  }

  if (requestUrl.pathname !== "/capture" || req.method !== "GET") {
    jsonResponse(res, 404, { ok: false, error: "not found" }, context);
    return;
  }

  if (workerToken) {
    const providedToken = readBearerToken(req);
    if (!providedToken || providedToken !== workerToken) {
      logEvent("info", "capture_auth_failed", {
        request_id: requestId,
        path: context.path,
      });
      jsonResponse(res, 401, { ok: false, error: "unauthorized" }, context);
      return;
    }
  }

  const rawTargetUrl = requestUrl.searchParams.get("url");
  if (!rawTargetUrl) {
    logEvent("info", "capture_validation_failed", {
      request_id: requestId,
      reason: "missing_url",
      path: context.path,
    });
    jsonResponse(res, 400, { ok: false, error: "missing url query parameter" }, context);
    return;
  }

  const validation = await validateTargetUrl(rawTargetUrl);
  if (validation.error) {
    logEvent("info", "capture_validation_failed", {
      request_id: requestId,
      reason: validation.reason || "validation_failed",
      error: validation.error,
      target_url: sanitizeUrlForLogs(rawTargetUrl),
    });
    jsonResponse(res, 400, { ok: false, error: validation.error }, context);
    return;
  }

  try {
    const image = await captureScreenshotAsDataUrl(validation.value, requestId);
    jsonResponse(res, 200, { ok: true, image }, context);
  } catch (error) {
    const classification = classifyCaptureError(error);
    logEvent("info", "capture_failed", {
      request_id: requestId,
      target_url: sanitizeUrlForLogs(rawTargetUrl),
      reason: "capture_failed",
      error_class: classification.errorClass,
      error_reason: classification.reason,
    });
    jsonResponse(res, 502, { ok: false, error: "failed to capture screenshot" }, context);
  }
});

server.listen(port, "0.0.0.0", () => {
  logEvent("info", "worker_listening", {
    host: "0.0.0.0",
    port,
    log_level: logLevel,
    log_preview_url_mode: logPreviewUrlMode,
  });
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
