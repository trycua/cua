import { getToken } from "../auth/keycloak";
import {
	isLocalVisualPreview,
	localVisualPreviewPath,
} from "../local-visual-preview";

export interface SavedCard {
	brand: string;
	last4: string;
	exp_month: number;
	exp_year: number;
}

export interface BillingSummary {
	payment_method_present: boolean;
	card: SavedCard | null;
}

export type UsageRangeMonths = 3 | 6 | 12;

export interface UsagePoint {
	period_start: string;
	period_end: string;
	amount: number;
	estimate: boolean;
}

export interface UsageBreakdownItem {
	name: string;
	amount: number;
	quantity: number;
	period_start: string;
	period_end: string;
}

export interface BillingUsage {
	currency: string;
	range_start: string;
	range_end: string;
	current_period_start: string | null;
	current_period_end: string | null;
	current_estimate: number;
	previous_period_amount: number;
	trend: UsagePoint[];
	breakdown: UsageBreakdownItem[];
}

async function billingRequest<T>(
	path: string,
	method: "GET" | "POST",
): Promise<T> {
	const token = await getToken();
	const response = await fetch(path, {
		method,
		headers: { Authorization: token ? `Bearer ${token}` : "" },
	});
	if (!response.ok)
		throw new Error(`billing request failed: ${response.status}`);
	return response.json() as Promise<T>;
}

export const billingApi = {
	summary: () =>
		isLocalVisualPreview()
			? Promise.resolve<BillingSummary>({
					payment_method_present: true,
					card: { brand: "visa", last4: "4242", exp_month: 12, exp_year: 2030 },
				})
			: billingRequest<BillingSummary>("/api/billing/summary", "GET"),
	setup: () =>
		isLocalVisualPreview()
			? Promise.resolve({
					url: localVisualPreviewPath("/settings?billing=setup-preview"),
				})
			: billingRequest<{ url: string }>("/api/billing/setup-session", "POST"),
	portal: () =>
		isLocalVisualPreview()
			? Promise.resolve({
					url: localVisualPreviewPath("/settings?billing=portal-preview"),
				})
			: billingRequest<{ url: string }>("/api/billing/portal-session", "POST"),
	usage: async (months: UsageRangeMonths) => {
		if (!isLocalVisualPreview()) {
			return billingRequest<BillingUsage>(
				`/api/billing/usage?months=${months}`,
				"GET",
			);
		}

		const state = new URLSearchParams(window.location.search).get(
			"cua-preview-state",
		);
		if (state === "error") throw new Error("Preview usage is unavailable");
		if (state === "loading") {
			await new Promise((resolve) => window.setTimeout(resolve, 2_000));
		}
		if (state === "empty") {
			return {
				currency: "usd",
				range_start: "2026-02-01T00:00:00Z",
				range_end: "2026-08-22T00:00:00Z",
				current_period_start: null,
				current_period_end: null,
				current_estimate: 0,
				previous_period_amount: 0,
				trend: [],
				breakdown: [],
			} satisfies BillingUsage;
		}

		const allTrend: UsagePoint[] = [
			{ period_start: "2026-03-01T00:00:00Z", period_end: "2026-04-01T00:00:00Z", amount: 18420, estimate: false },
			{ period_start: "2026-04-01T00:00:00Z", period_end: "2026-05-01T00:00:00Z", amount: 22780, estimate: false },
			{ period_start: "2026-05-01T00:00:00Z", period_end: "2026-06-01T00:00:00Z", amount: 21410, estimate: false },
			{ period_start: "2026-06-01T00:00:00Z", period_end: "2026-07-01T00:00:00Z", amount: 28640, estimate: false },
			{ period_start: "2026-07-01T00:00:00Z", period_end: "2026-08-01T00:00:00Z", amount: 31920, estimate: false },
			{ period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z", amount: 24680, estimate: true },
		];
		return {
			currency: "usd",
			range_start: "2026-02-22T00:00:00Z",
			range_end: "2026-08-22T00:00:00Z",
			current_period_start: "2026-08-01T00:00:00Z",
			current_period_end: "2026-09-01T00:00:00Z",
			current_estimate: 24680,
			previous_period_amount: 31920,
			trend: allTrend.slice(-months),
			breakdown: [
				{ name: "Linux desktop runtime", amount: 13240, quantity: 368, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
				{ name: "Windows desktop runtime", amount: 6860, quantity: 98, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
				{ name: "Android emulator runtime", amount: 3890, quantity: 81, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
				{ name: "Persistent storage", amount: 690, quantity: 230, period_start: "2026-08-01T00:00:00Z", period_end: "2026-09-01T00:00:00Z" },
			],
		} satisfies BillingUsage;
	},
};
