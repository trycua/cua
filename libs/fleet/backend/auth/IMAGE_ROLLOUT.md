# Initial Image API rollout

`ImageRolloutPolicy` is a mandatory shared conjunct on `K8sRoutePolicy` and
`ImageUploadsRoutePolicy`. It requires the existing `authz.is_admin` decision for:

- Image collection POST and item PATCH/DELETE through
  `/api/k8s/apis/images.cua.ai/v1alpha1/namespaces/{namespace}/images`.
- POST `/api/image-uploads/presign`, before the upload handler or object store runs.

This reuses `/feature-flags/cyclops-cs/admin-subs` as a user-subject cohort, not a
namespace list. User API keys are evaluated using the owner subject resolved by
`TokenAuthMiddleware`; already-authorized GitHub delegation from an admin owner
uses the same existing subject semantics. Namespace keys do not acquire owner
identity or Image proxy access. No membership or principal-family rules change.
Membership uses the existing authorization flag cache (up to one minute); absent
membership does not authorize an Image write.

The gate is composed with `All`, not an alternative allow branch. Resource and
principal allowlists, namespace ownership, billing admission (including its
existing admin exemption), and Image body restrictions still apply. Presign
namespace authorization remains in the handler. Read/list/watch behavior and
other resources and routes are unchanged; unsupported methods and subresources
remain denied by their existing policies/router.

This restricts the initial API rollout only. It does not activate the controller,
change `BuildDisabled`, deploy a preparation worker, or make a feature flag
sufficient to start builds. Raw Kubernetes API admission is a separate unresolved
boundary; this HTTP authorization policy is not an admission webhook and cannot
gate callers that bypass the Cyclops API. Previously issued upload URLs are not
revoked by this policy.

Regression coverage includes the Go Image authorization matrices, the real
presign route middleware plus handler, Rego scope tests, and the route
characterization table. The table's expected changes are only nonadmin Image
POST/DELETE decisions and nonadmin upload POST decisions (its Image PATCH rows
already lack the required merge-patch content type and remain denied).
