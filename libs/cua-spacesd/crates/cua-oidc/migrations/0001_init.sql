-- SPDX-License-Identifier: FSL-1.1-MIT
-- Cua OIDC issuer: connections (metadata only), the append-only audit log,
-- and the device-proof replay cache.

CREATE TABLE oidc_connections (
    id                text PRIMARY KEY,
    account           text   NOT NULL,
    cloud             text   NOT NULL,
    -- The naming identifier (AWS account id, GCP project number, ...).
    primary_id        text   NOT NULL,
    status            text   NOT NULL CHECK (status IN ('pending', 'connected', 'revoked')),
    version           bigint NOT NULL,
    -- The connection as JSON, without the sealed token.
    record            jsonb  NOT NULL,
    -- Sealed API token: ciphertext only the account's devices can open.
    sealed_ciphertext bytea,
    sealed_generation bigint,
    created_at        bigint NOT NULL,
    updated_at        bigint NOT NULL
);

CREATE INDEX oidc_connections_account ON oidc_connections (account, created_at);

-- One live connection per account, cloud and naming identifier.
CREATE UNIQUE INDEX oidc_connections_live
    ON oidc_connections (account, cloud, primary_id)
    WHERE status <> 'revoked';

CREATE TABLE oidc_audit (
    seq        bigserial PRIMARY KEY,
    at         bigint NOT NULL,
    account    text   NOT NULL,
    kind       text   NOT NULL,
    connection text,
    device     text,
    agent      text,
    "user"     text,
    purpose    text,
    audience   text,
    jti        text,
    code       text
);

CREATE INDEX oidc_audit_account ON oidc_audit (account, seq DESC);
CREATE INDEX oidc_audit_connection ON oidc_audit (account, connection, seq DESC);

CREATE FUNCTION oidc_audit_append_only() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'oidc_audit is append-only';
END
$$;

CREATE TRIGGER oidc_audit_no_update_or_delete
    BEFORE UPDATE OR DELETE ON oidc_audit
    FOR EACH ROW EXECUTE FUNCTION oidc_audit_append_only();

CREATE TRIGGER oidc_audit_no_truncate
    BEFORE TRUNCATE ON oidc_audit
    FOR EACH STATEMENT EXECUTE FUNCTION oidc_audit_append_only();

CREATE TABLE oidc_proofs (
    key        text PRIMARY KEY,
    expires_at bigint NOT NULL
);

CREATE INDEX oidc_proofs_expiry ON oidc_proofs (expires_at);
