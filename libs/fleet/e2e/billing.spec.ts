import { expect, test, type Page } from "@playwright/test";
import { mockAuth } from "./fixtures/mock-api";

async function setBillingFlag(
	page: Page,
	billing: boolean,
	admin = false,
	usagePricing = { vcpu_hour_usd: 0.044625, memory_gib_hour_usd: 0.0223125 },
) {
	await page.unroute("**/api/config");
	await page.route("**/api/config", (route) =>
		route.fulfill({
			contentType: "application/json",
			body: JSON.stringify({ admin, billing, usage_pricing: usagePricing }),
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

test("usage appears without a billing flag or admin badge", async ({
	page,
}) => {
	await setBillingFlag(page, false);
	await page.goto("/pools");
	await expect(page.getByRole("link", { name: "API keys" })).toBeVisible();
	await expect(page.getByRole("link", { name: "Usage", exact: true })).toBeVisible();
	await expect(page.getByText("Admin", { exact: true })).toHaveCount(0);
});

test("usage shows reserved resources and calculated cost by pool", async ({ page }) => {
	await setBillingFlag(page, true);
	await mockResourceUsage(page);
	await page.goto("/usage");
	await expect(page.getByRole("button", { name: "Usage history range Last 24 hours" })).toBeVisible();
	const pricing = page.locator(".usage-pricing");
	await expect(pricing.getByText("CPU: $0.044625/vCPU/hour")).toBeVisible();
	await expect(pricing.getByText("Memory: $0.0223125/GB/hour")).toBeVisible();
	await expect(page.getByText("Cost", { exact: true }).first()).toBeVisible();
	const summary = page.getByLabel("Usage summary");
	await expect(summary.getByText("$43.51")).toBeVisible();
	await expect(summary.getByText("Timeframe", { exact: true })).toHaveCount(0);
	await expect(summary.getByText("Data status", { exact: true })).toHaveCount(0);
	await expect(page.getByRole("columnheader")).toHaveCount(4);
	await expect(page.getByRole("cell", { name: "prod-web-fleet" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "240 core-h" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "960 GiB-h" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "$32.13" })).toBeVisible();
	await expect(page.getByText("OpenCost", { exact: false })).toHaveCount(0);
	await expect(page.locator(".usage-trend")).toHaveCount(0);
});

test("usage cost uses pricing returned by api config", async ({ page }) => {
	await setBillingFlag(page, false, false, {
		vcpu_hour_usd: 0.1,
		memory_gib_hour_usd: 0.2,
	});
	await mockResourceUsage(page);
	await page.goto("/usage");
	const pricing = page.locator(".usage-pricing");
	await expect(pricing.getByText("CPU: $0.1/vCPU/hour")).toBeVisible();
	await expect(pricing.getByText("Memory: $0.2/GB/hour")).toBeVisible();
	await expect(page.getByLabel("Usage summary").getByText("$291.00")).toBeVisible();
	await expect(page.getByRole("cell", { name: "$216.00" })).toBeVisible();
});

test("usage subject lookup is visible only to admins", async ({ page }) => {
	await setBillingFlag(page, true);
	await mockResourceUsage(page);
	await page.goto("/usage");
	await expect(
		page.getByRole("heading", { name: "View usage by subject" }),
	).toHaveCount(0);
});

test("admin can view and reset usage for another subject", async ({ page }) => {
	const targetSubject = "30a53246-881d-4f1a-8005-979f2a07933e";
	const requestedSubjects: Array<string | null> = [];
	const targetOverview = {
		...usageOverview,
		pools: [
			{
				id: "preview:target-pool",
				name: "target-pool",
				cpu: { consumed: 1, provisioned: 12 },
				memory: { consumed: 4, provisioned: 48 },
				cost_usd: 0,
			},
		],
	};

	await setBillingFlag(page, true, true);
	await page.route("**/api/usage/overview**", (route) => {
		const subject = new URL(route.request().url()).searchParams.get("subject");
		requestedSubjects.push(subject);
		return route.fulfill({
			json: subject === targetSubject ? targetOverview : usageOverview,
		});
	});

	await page.goto("/usage");
	await expect(
		page.getByRole("heading", { name: "View usage by subject" }),
	).toBeVisible();
	await page
		.getByRole("textbox", { name: "Keycloak user subject ID" })
		.fill(`  ${targetSubject}  `);
	await page.getByRole("button", { name: "View usage" }).click();

	await expect(page.getByText(`Showing usage for ${targetSubject}`)).toBeVisible();
	await expect(page.getByRole("cell", { name: "target-pool" })).toBeVisible();
	await expect.poll(() => requestedSubjects.at(-1)).toBe(targetSubject);

	await page.getByRole("button", { name: "Reset to my usage" }).click();
	// The default-state hint was removed — no description once the
	// subject lookup is reset (see FormField in BillingUsage.tsx).
	await expect(page.getByText("Showing usage for")).toHaveCount(0);
	await expect(page.getByRole("cell", { name: "prod-web-fleet" })).toBeVisible();
	await expect.poll(() => requestedSubjects.at(-1)).toBeNull();
});

test("visual preview shows resource usage without authentication", async ({ page }) => {
	await page.goto("/usage?cua-visual-preview");
	await expect(page.getByLabel("Usage summary").getByText("$372.71")).toBeVisible();
	await expect(page.getByRole("button", { name: "Usage history range Last 24 hours" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "browser-agent-fleet" })).toBeVisible();
	await expect(page.getByRole("cell", { name: "$179.93" })).toBeVisible();
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
