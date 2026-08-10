import { getToken } from "../auth/keycloak";

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
	summary: () => billingRequest<BillingSummary>("/api/billing/summary", "GET"),
	setup: () =>
		billingRequest<{ url: string }>("/api/billing/setup-session", "POST"),
	portal: () =>
		billingRequest<{ url: string }>("/api/billing/portal-session", "POST"),
};
