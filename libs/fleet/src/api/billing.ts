import { getToken } from "../auth/keycloak";
import {
	getLocalVisualPreviewBillingUsage,
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
	/**
	 * True when creating a pool (or any custom resource) would be denied by
	 * the backend's card-admission policy until a payment card is added.
	 * Optional so older backends without the field keep parsing.
	 */
	pool_create_card_required?: boolean;
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
	body?: unknown,
): Promise<T> {
	const token = await getToken();
	const headers: Record<string, string> = {
		Authorization: token ? `Bearer ${token}` : "",
	};
	if (body !== undefined) headers["Content-Type"] = "application/json";
	const response = await fetch(path, {
		method,
		headers,
		body: body === undefined ? undefined : JSON.stringify(body),
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
					pool_create_card_required: false,
				})
			: billingRequest<BillingSummary>("/api/billing/summary", "GET"),
	setup: () =>
		isLocalVisualPreview()
			? Promise.resolve({
					url: localVisualPreviewPath("/settings?billing=setup-preview"),
				})
			: billingRequest<{ url: string }>("/api/billing/setup-session", "POST"),
	completeSetup: (sessionID: string) =>
		billingRequest<{ applied: boolean }>(
			"/api/billing/setup-session/complete",
			"POST",
			{ session_id: sessionID },
		),
	portal: () =>
		isLocalVisualPreview()
			? Promise.resolve({
					url: localVisualPreviewPath("/settings?billing=portal-preview"),
				})
			: billingRequest<{ url: string }>("/api/billing/portal-session", "POST"),
	usage: (months: UsageRangeMonths) => {
		if (isLocalVisualPreview()) {
			return getLocalVisualPreviewBillingUsage(months);
		}
		return billingRequest<BillingUsage>(
			`/api/billing/usage?months=${months}`,
			"GET",
		);
	},
};
