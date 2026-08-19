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
};
