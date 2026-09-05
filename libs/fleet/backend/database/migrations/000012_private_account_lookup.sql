CREATE SCHEMA account_lookup_private AUTHORIZATION CURRENT_USER;
REVOKE ALL ON SCHEMA account_lookup_private FROM PUBLIC;
GRANT USAGE ON SCHEMA account_lookup_private TO cyclops_app;

CREATE TABLE account_lookup_private.mapping (
    realm text NOT NULL CHECK (realm <> ''),
    key_id text NOT NULL CHECK (key_id <> ''),
    pseudonym text NOT NULL CHECK (pseudonym <> ''),
    subject text NOT NULL CHECK (subject <> ''),
    PRIMARY KEY (realm, key_id, pseudonym)
);

-- Completion means a successful observed scan from offset zero, not a snapshot.
CREATE TABLE account_lookup_private.backfill (
    realm text NOT NULL,
    key_id text NOT NULL,
    observed_scan_completed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (realm, key_id)
);

CREATE TABLE account_lookup_private.audit (
    actor text NOT NULL CHECK (actor <> ''),
    occurred_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    outcome text NOT NULL CHECK (outcome IN (
        'found', 'not_found', 'mapping_missing', 'account_missing',
        'invalid', 'unavailable', 'rate_limited', 'forbidden'
    ))
);

CREATE TABLE account_lookup_private.rate_limit (
    actor text PRIMARY KEY CHECK (actor <> ''),
    attempts timestamptz[] NOT NULL DEFAULT '{}'
);

REVOKE ALL ON ALL TABLES IN SCHEMA account_lookup_private FROM PUBLIC;
GRANT SELECT, INSERT ON account_lookup_private.mapping TO cyclops_app;
GRANT SELECT, INSERT, UPDATE ON account_lookup_private.backfill TO cyclops_app;
GRANT INSERT ON account_lookup_private.audit TO cyclops_app;
GRANT SELECT, INSERT, UPDATE ON account_lookup_private.rate_limit TO cyclops_app;
