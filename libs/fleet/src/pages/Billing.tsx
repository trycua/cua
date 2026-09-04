// Billing section rendered inside Settings. Card collection and management
// happen on Stripe-hosted pages; Cyclops never handles card details.

import Alert from "@cloudscape-design/components/alert";
import Box from "@cloudscape-design/components/box";
import Container from "@cloudscape-design/components/container";
import Header from "@cloudscape-design/components/header";
import SpaceBetween from "@cloudscape-design/components/space-between";
import Spinner from "@cloudscape-design/components/spinner";
import { useEffect, useState } from "react";
import { CuaButton } from "../components/CuaButton";
import { billingApi, type BillingSummary } from "../api/billing";

function cardBrand(brand: string) {
	return brand ? brand[0].toUpperCase() + brand.slice(1) : "Card";
}

export function BillingSettings() {
	const [summary, setSummary] = useState<BillingSummary | null>(null);
	const [error, setError] = useState(false);
	const [redirecting, setRedirecting] = useState(false);

	useEffect(() => {
		billingApi
			.summary()
			.then(setSummary)
			.catch(() => setError(true));
	}, []);

	const redirect = async (kind: "setup" | "portal") => {
		setRedirecting(true);
		setError(false);
		try {
			const { url } =
				kind === "setup" ? await billingApi.setup() : await billingApi.portal();
			window.location.assign(url);
		} catch {
			setError(true);
			setRedirecting(false);
		}
	};

	if (error) {
		return (
			<Alert type="error" header="Billing is temporarily unavailable">
				Try again later. No billing changes were made.
			</Alert>
		);
	}

	return (
		<SpaceBetween size="l">
			<Container
				header={
					<Header
						variant="h2"
						description="Save one card securely with Stripe for future automatic usage charges."
					>
						Payment method
					</Header>
				}
				footer={
					summary && (
						<CuaButton
							tone="primary"
							loading={redirecting}
							onClick={() =>
								redirect(summary.payment_method_present ? "portal" : "setup")
							}
						>
							{summary.payment_method_present
								? "Manage payment method"
								: "Add payment method"}
						</CuaButton>
					)
				}
			>
				{!summary ? (
					<Spinner />
				) : summary.payment_method_present && summary.card ? (
					<SpaceBetween size="xs">
						<Box variant="h3">
							{cardBrand(summary.card.brand)} ending in {summary.card.last4}
						</Box>
						<Box color="text-body-secondary">
							Expires {String(summary.card.exp_month).padStart(2, "0")}/
							{summary.card.exp_year}
						</Box>
					</SpaceBetween>
				) : (
					<SpaceBetween size="s">
						<Box variant="h3">No payment method saved</Box>
						<Box color="text-body-secondary">
							By saving this card, you authorize Fleet to charge it
							automatically for future usage according to the applicable pricing
							and terms.
						</Box>
					</SpaceBetween>
				)}
			</Container>
		</SpaceBetween>
	);
}
