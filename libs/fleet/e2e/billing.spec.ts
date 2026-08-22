import { expect, test, type Page } from "@playwright/test";
import { mockAuth } from "./fixtures/mock-api";

async function setBillingFlag(page: Page, billing: boolean) {
	await page.unroute("**/api/config");
	await page.route("**/api/config", (route) =>
		route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({ admin: false, billing }),
		}),
	);
}

test.beforeEach(async ({ page }) => {
	await mockAuth(page);
});

const usageOverview = {
	data_as_of: "2026-08-22T12:00:00Z",
	partial: false,
	pools: [
		{
			id: "preview:prod-web-fleet",
			name: "prod-web-fleet",
			cpu: { consumed: 120.5, provisioned: 240 },
			memory: { consumed: 480, provisioned: 960 },
			cost_usd: 2.75,
		},
		{
			id: "preview:batch-jobs-fleet",
			name: "batch-jobs-fleet",
			cpu: { consumed: 60, provisioned: 90 },
			memory: { consumed: 220, provisioned: 330 },
			cost_usd: 0.89,
		},
	],
};

async function mockResourceUsage(page: Page) {
	await page.route("**/api/usage/overview**", (route) =>
		route.fulfill({ json: usageOverview }),
	);
}

test("usage appears in navigation when billing is enabled", async ({
	page,
}) => {
	await setBillingFlag(page, true);
	await page.goto("/pools");
	await expect(page.getByRole("link", { name: "User API keys" })).toBeVisible();
	await expect(page.getByRole("link", { name: "Usage", exact: true })).toBeVisible();
});

test("usage shows reserved resources and pool costs", async ({ page }) => {
	await setBillingFlag(page, true);
	await mockResourceUsage(page);
	await page.goto("/usage");
	await expect(page.getByRole("button", { name: "Usage history range Last 24 hours" })).toBeVisible();
	await expect(page.getByText("OpenCost incurred", { exact: true }).first()).toBeVisible();
	const summary = page.getByLabel("Usage summary");
	await expect(summary.getByText("$3.64")).toBeVisible();
	await expect(summary.getByText("Timeframe", { exact: true })).toHaveCount(0);
	await expect(summary.getByText("Data status", { exact: true })).toHaveCount(0);
	await expect(page.getByRole("columnheader")).toHaveCount(4);
	await expect(page.getByRole("cell", { name: "prod-web-fleet" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "$2.75" })).toBeVisible();
	await expect(page.locator(".usage-trend")).toHaveCount(0);
});

test("visual preview shows resource usage without authentication", async ({ page }) => {
	await page.goto("/usage?cua-visual-preview");
	await expect(page.getByLabel("Usage summary").getByText("$37.47")).toBeVisible();
	await expect(page.getByRole("button", { name: "Usage history range Last 24 hours" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "browser-agent-fleet" })).toBeVisible();
});

test("usage has a useful empty state", async ({ page }) => {
	await setBillingFlag(page, true);
	await page.route("**/api/usage/overview**", (route) =>
		route.fulfill({
			json: { data_as_of: usageOverview.data_as_of, partial: false, pools: [] },
		}),
	);
	await page.goto("/usage");
	await expect(page.getByText("No resource usage yet")).toBeVisible();
	await expect(page.getByText(/pools begin reporting activity/i)).toBeVisible();
});

test("usage can retry a failed load", async ({ page }) => {
	await setBillingFlag(page, true);
	await page.route("**/api/usage/overview**", (route) =>
		route.fulfill({ status: 502, json: { error: "unavailable" } }),
	);
	await page.goto("/usage");
	await expect(page.getByText("Usage is temporarily unavailable")).toBeVisible();
	await expect(page.getByRole("button", { name: "Try again" })).toBeVisible();
});

test("settings hides billing when the flag is off", async ({ page }) => {
	await setBillingFlag(page, false);
	await page.goto("/settings");
	await expect(page.getByRole("heading", { name: "Account" })).toBeVisible();
	await expect(
		page.getByRole("heading", { name: "Payment method" }),
	).toHaveCount(0);
	await expect(
		page.getByRole("heading", { name: "Recent invoices" }),
	).toHaveCount(0);
});

test("settings explains automatic charges before saving a card", async ({
	page,
}) => {
	await setBillingFlag(page, true);
	await page.route("**/api/billing/summary", (route) =>
		route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({ payment_method_present: false, card: null }),
		}),
	);
	await page.route("**/api/billing/setup-session", async (route) => {
		expect(route.request().method()).toBe("POST");
		await route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({ url: "/settings?setup=complete" }),
		});
	});
	await page.goto("/settings");
	await expect(
		page.getByText(
			"By saving this card, you authorize Fleet to charge it automatically for future usage according to the applicable pricing and terms.",
		),
	).toBeVisible();
	const redirect = page.waitForURL(/\/settings\?setup=complete$/);
	await page.getByRole("button", { name: "Add payment method" }).click();
	await redirect;
});

test("settings displays the saved default card", async ({ page }) => {
	await setBillingFlag(page, true);
	await page.route("**/api/billing/summary", (route) =>
		route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({
				payment_method_present: true,
				card: { brand: "visa", last4: "4242", exp_month: 12, exp_year: 2030 },
			}),
		}),
	);
	await page.route("**/api/billing/portal-session", async (route) => {
		expect(route.request().method()).toBe("POST");
		await route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({ url: "/settings?portal=complete" }),
		});
	});
	await page.goto("/settings");
	await expect(page.getByText("Visa ending in 4242")).toBeVisible();
	await expect(page.getByText("Expires 12/2030")).toBeVisible();
	const portalRedirect = page.waitForURL(/\/settings\?portal=complete$/);
	await page.getByRole("button", { name: "Manage payment method" }).click();
	await portalRedirect;
	await expect(
		page.getByRole("heading", { name: "Recent invoices" }),
	).toHaveCount(0);
});

test("settings shows a billing load error", async ({ page }) => {
	await setBillingFlag(page, true);
	await page.route("**/api/billing/summary", (route) =>
		route.fulfill({
			status: 502,
			contentType: "application/json",
			body: '{"error":"unavailable"}',
		}),
	);
	await page.goto("/settings");
	await expect(
		page.getByText("Billing is temporarily unavailable"),
	).toBeVisible();
	await expect(page.getByRole("heading", { name: "Account" })).toBeVisible();
});
