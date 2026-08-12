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

test("billing has no navigation link even when the flag is on", async ({
	page,
}) => {
	await setBillingFlag(page, true);
	await page.route("**/api/billing/summary", (route) =>
		route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({ payment_method_present: false, card: null }),
		}),
	);
	await page.goto("/pools");
	await expect(page.getByRole("link", { name: "User API keys" })).toBeVisible();
	await expect(page.getByRole("link", { name: "Billing" })).toHaveCount(0);
});

test("the old /billing route redirects to settings", async ({ page }) => {
	await setBillingFlag(page, true);
	await page.route("**/api/billing/summary", (route) =>
		route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({ payment_method_present: false, card: null }),
		}),
	);
	await page.goto("/billing");
	await expect(page.getByRole("heading", { name: "Account" })).toBeVisible();
	await expect(page).toHaveURL(/\/settings$/);
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
