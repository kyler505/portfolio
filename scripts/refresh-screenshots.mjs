const endpoint = process.env.SCREENSHOT_REFRESH_ENDPOINT;
const token = process.env.SCREENSHOT_REFRESH_TOKEN;

if (!endpoint) {
  console.error("missing SCREENSHOT_REFRESH_ENDPOINT");
  process.exit(1);
}

if (!token) {
  console.error("missing SCREENSHOT_REFRESH_TOKEN");
  process.exit(1);
}

const response = await fetch(endpoint, {
  method: "POST",
  headers: {
    authorization: `Bearer ${token}`,
  },
});

const body = await response.text();
if (!response.ok) {
  console.error(`refresh request failed (${response.status}): ${body}`);
  process.exit(1);
}

console.log(`refresh request succeeded (${response.status}): ${body}`);
