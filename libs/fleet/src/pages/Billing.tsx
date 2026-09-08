// Billing section rendered inside Settings. Card collection and management
// happen on Stripe-hosted pages; Cyclops never handles card details.

import Alert from "@cloudscape-design/components/alert";
import Box from "@cloudscape-design/components/box";
import Container from "@cloudscape-design/components/container";
import Header from "@cloudscape-design/components/header";
import SpaceBetween from "@cloudscape-design/components/space-between";
import Spinner from "@cloudscape-design/components/spinner";
import { useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { CuaButton } from "../components/CuaButton";
import { billingApi, type BillingSummary } from "../api/billing";

function cardBrand(brand: string) {
	return brand ? brand[0].toUpperCase() + brand.slice(1) : "Card";
}

export function BillingSettings() {
	const location = useLocation();
	const navigate = useNavigate();
	const [summary, setSummary] = useState<BillingSummary | null>(null);
	const [error, setError] = useState(false);
	const [redirecting, setRedirecting] = useState(false);
	const [setupComplete, setSetupComplete] = useState(false);
	const completionAttempt = useRef<string | null>(null);

	useEffect(() => {
		const params = new URLSearchParams(location.search);
		const sessionID = params.get("session_id");
		if (params.get("checkout") !== "success" || !sessionID) {
			billingApi
				.summary()
				.then(setSummary)
				.catch(() => setError(true));
			return;
		}
		if (completionAttempt.current === sessionID) return;
		completionAttempt.current = sessionID;
		setSummary(null);
		setRedirecting(true);
		setError(false);
		billingApi
			.completeSetup(sessionID)
			.then(() => billingApi.summary())
			.then((nextSummary) => {
				if (!nextSummary.payment_method_present) {
					throw new Error("payment method was not confirmed");
				}
				setSummary(nextSummary);
				setSetupComplete(true);
			})
			.catch(() => setError(true))
			.finally(() => {
				params.delete("checkout");
				params.delete("session_id");
				const search = params.toString();
				navigate(
					{ pathname: location.pathname, search: search ? `?${search}` : "" },
					{ replace: true },
				);
				setRedirecting(false);
			});
	}, [location.pathname, location.search, navigate]);

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
				Refresh and try again. Card details remain securely with Stripe.
			</Alert>
		);
	}

	return (
		<SpaceBetween size="l">
			{setupComplete && (
				<Alert type="success" header="Payment method saved">
					Your card is ready for Fleet usage charges.
				</Alert>
			)}
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
					<SpaceBetween direction="horizontal" size="xs">
						<Spinner />
						<Box>{redirecting ? "Confirming payment method" : "Loading"}</Box>
					</SpaceBetween>
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
