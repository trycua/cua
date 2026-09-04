CREATE TABLE public.signed_service_urls (
    id uuid PRIMARY KEY,
    namespace text NOT NULL CHECK (namespace <> ''),
    claim_name text NOT NULL CHECK (claim_name <> ''),
    sandbox_name text NOT NULL CHECK (sandbox_name <> ''),
    service_name text NOT NULL CHECK (service_name <> ''),
    logical_service text NOT NULL CHECK (logical_service <> ''),
    label text CHECK (label IS NULL OR (label <> '' AND octet_length(label) <= 120)),
    creator_sub text NOT NULL CHECK (creator_sub <> ''),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    CHECK (expires_at >= created_at + interval '1 minute'),
    CHECK (expires_at <= created_at + interval '24 hours'),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE INDEX signed_service_urls_claim_created_idx
    ON public.signed_service_urls (namespace, claim_name, created_at DESC, id);
CREATE INDEX signed_service_urls_expiry_idx
    ON public.signed_service_urls (expires_at)
    WHERE revoked_at IS NULL;

REVOKE ALL ON TABLE public.signed_service_urls FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE ON TABLE public.signed_service_urls TO cyclops_app;
