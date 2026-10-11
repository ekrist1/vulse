const assert = require("node:assert/strict");

// Signs `page` in as a new admin on the isolated test server and returns a
// suffix that keeps this run's collection names unique in the shared
// workspace. The server must start with VULSE_SETUP_TOKEN set to the same
// value; the first suite to run creates the bootstrap admin.
async function signInFreshAdmin(page, base, prefix) {
  const token = process.env.VULSE_SETUP_TOKEN || "vulse-browser-tests-setup";
  const admin = { email: "admin@vulse.test", handle: "admin", password: "test-admin-password" };
  let response = await page.request.post(base + "/api/v1/auth/setup", { data: { token, ...admin } });
  if (response.status() === 409) {
    response = await page.request.post(base + "/api/v1/auth/login", { data: { email: admin.email, password: admin.password } });
  }
  assert.ok(response.ok(), `bootstrap admin sign-in failed: ${response.status()} ${await response.text()}`);
  const run = Date.now().toString(36);
  const handle = prefix + run;
  const invite = await page.request.post(base + "/api/v1/invites", { data: { email: handle + "@example.com", role: "admin" } });
  assert.equal(invite.status(), 201);
  const { path } = await invite.json();
  await page.goto(base + path);
  await page.getByLabel("Handle").fill(handle);
  await page.getByLabel("Password").fill("test-password");
  await page.getByRole("button", { name: "Create account", exact: true }).click();
  await page.getByRole("heading", { name: "A home for your data." }).waitFor();
  return { handle, run };
}

// Accepts the in-app confirmation modal, optionally checking its button label.
async function confirmModal(page, label) {
  const dialog = page.getByRole("dialog");
  const accept = label ? dialog.getByRole("button", { name: label, exact: true }) : dialog.locator(".confirm-accept");
  await accept.click();
  await dialog.waitFor({ state: "detached" });
}

module.exports = { signInFreshAdmin, confirmModal };
