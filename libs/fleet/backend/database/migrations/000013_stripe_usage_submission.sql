-- Stripe usage submission ledger (CUA-1158).
--
-- Records every meter event the usage submitter archives and sends so that
-- replays, revisions, and out-of-window corrections are provable locally.
-- Stripe's identifier dedupe is only guaranteed for 24 hours; this ledger is
-- the durable exactly-once record. Design:
-- docs/superpowers/specs/2026-09-16-cua-1158-stripe-usage-submitter-design.md
--
-- Touches no existing table. Creates two empty tables and one login role.

CREATE ROLE cyclops_submitter LOGIN NOINHERIT NOCREATEROLE NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS;

SET LOCAL ROLE billing_meter_owner;

CREATE TABLE billing_meter.stripe_submission_batch (
    batch_id uuid PRIMARY KEY,
    cluster_id text NOT NULL CHECK (cluster_id <> ''),
    cutover_hour timestamptz NOT NULL CHECK (date_trunc('hour', cutover_hour) = cutover_hour),
    window_start timestamptz NOT NULL CHECK (date_trunc('hour', window_start) = window_start),
    window_end timestamptz NOT NULL CHECK (date_trunc('hour', window_end) = window_end),
    dry_run boolean NOT NULL,
    archive_bucket text NOT NULL CHECK (archive_bucket <> ''),
    archive_key text NOT NULL UNIQUE CHECK (archive_key <> ''),
    archive_sha256 text CHECK (archive_sha256 IS NULL OR archive_sha256 ~ '^[0-9a-f]{64}$'),
    submission_count integer NOT NULL CHECK (submission_count >= 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    archived_at timestamptz,
    completed_at timestamptz,
    CHECK (window_end > window_start),
    CHECK (window_start >= cutover_hour),
    CHECK ((archived_at IS NULL) = (archive_sha256 IS NULL)),
    CHECK (completed_at IS NULL OR archived_at IS NOT NULL)
);

CREATE TABLE billing_meter.stripe_submission (
    submission_id uuid PRIMARY KEY,
    batch_id uuid NOT NULL REFERENCES billing_meter.stripe_submission_batch(batch_id),
    capsule_tenant text NOT NULL CHECK (capsule_tenant <> ''),
    hour_start timestamptz NOT NULL CHECK (date_trunc('hour', hour_start) = hour_start),
    meter text NOT NULL CHECK (meter IN ('cua_vcpu_hours', 'cua_gib_hours')),
    seq integer NOT NULL CHECK (seq > 0),
    identifier text NOT NULL UNIQUE CHECK (identifier <> '' AND length(identifier) <= 100),
    stripe_customer_id text CHECK (stripe_customer_id IS NULL OR stripe_customer_id <> ''),
    quantity numeric(20, 6) NOT NULL CHECK (quantity >= 0),
    fact_set_sha256 text NOT NULL CHECK (fact_set_sha256 ~ '^[0-9a-f]{64}$'),
    status text NOT NULL CHECK (status IN ('archived', 'sent', 'dry_run', 'failed', 'out_of_window', 'unmapped')),
    stripe_event_id text CHECK (stripe_event_id IS NULL OR stripe_event_id <> ''),
    error_code text,
    error_message text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    sent_at timestamptz,
    UNIQUE (capsule_tenant, hour_start, meter, seq),
    CHECK (status <> 'sent' OR (stripe_event_id IS NOT NULL AND sent_at IS NOT NULL AND stripe_customer_id IS NOT NULL)),
    CHECK (status <> 'unmapped' OR stripe_customer_id IS NULL),
    CHECK (status NOT IN ('failed', 'out_of_window') OR error_code IS NOT NULL)
);

CREATE INDEX stripe_submission_tenant_hour_meter_idx
    ON billing_meter.stripe_submission (capsule_tenant, hour_start, meter, seq DESC);
CREATE INDEX stripe_submission_status_idx
    ON billing_meter.stripe_submission (status, hour_start);

-- Status moves forward only. 'archived' is the only non-terminal state.
CREATE FUNCTION billing_meter.enforce_stripe_submission_transition()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.status <> 'archived' THEN
        RAISE EXCEPTION 'stripe submission % is terminal in status %', OLD.identifier, OLD.status;
    END IF;
    IF NEW.status = 'archived' THEN
        RAISE EXCEPTION 'stripe submission % cannot return to archived', OLD.identifier;
    END IF;
    IF NEW.submission_id <> OLD.submission_id OR NEW.batch_id <> OLD.batch_id
        OR NEW.capsule_tenant <> OLD.capsule_tenant OR NEW.hour_start <> OLD.hour_start
        OR NEW.meter <> OLD.meter OR NEW.seq <> OLD.seq OR NEW.identifier <> OLD.identifier
        OR NEW.quantity <> OLD.quantity OR NEW.fact_set_sha256 <> OLD.fact_set_sha256
        OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'stripe submission % identity and quantity are immutable', OLD.identifier;
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION billing_meter.reject_stripe_submission_removal()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'billing_meter stripe submissions are append-only';
END
$$;

CREATE TRIGGER stripe_submission_transition
BEFORE UPDATE ON billing_meter.stripe_submission
FOR EACH ROW EXECUTE FUNCTION billing_meter.enforce_stripe_submission_transition();
CREATE TRIGGER stripe_submission_no_removal
BEFORE DELETE OR TRUNCATE ON billing_meter.stripe_submission
FOR EACH STATEMENT EXECUTE FUNCTION billing_meter.reject_stripe_submission_removal();
CREATE TRIGGER stripe_submission_batch_no_removal
BEFORE DELETE OR TRUNCATE ON billing_meter.stripe_submission_batch
FOR EACH STATEMENT EXECUTE FUNCTION billing_meter.reject_stripe_submission_removal();

-- Migration 000010's default privileges grant k8s_metabase SELECT on these
-- tables, matching the repo invariant that Metabase reads every billing_meter
-- relation (reconciliation dashboards). The migrator's expected reporting ACL
-- list includes them.
REVOKE ALL ON TABLE billing_meter.stripe_submission, billing_meter.stripe_submission_batch FROM PUBLIC;
REVOKE ALL ON FUNCTION billing_meter.enforce_stripe_submission_transition() FROM PUBLIC;
REVOKE ALL ON FUNCTION billing_meter.reject_stripe_submission_removal() FROM PUBLIC;

GRANT USAGE ON SCHEMA billing_meter TO cyclops_submitter;
GRANT SELECT ON TABLE billing_meter.reservation_hour_current, billing_meter.reservation_hour_collection_current TO cyclops_submitter;
GRANT SELECT, INSERT, UPDATE ON TABLE billing_meter.stripe_submission, billing_meter.stripe_submission_batch TO cyclops_submitter;
RESET ROLE;
