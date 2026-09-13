# Image file uploads

Use `CyclopsClient.upload_image_file(namespace, name, contents)` with a native
HTTP client (or the browser constructor). `name` is a basename; `contents` is the
complete file as bytes, so callers must budget memory for it.

See the [Python image-build example](../sdk-bindings/examples/python/IMAGE_BUILD.md)
for file mappings, Image creation, and generation-pinned status polling.
Initial infrastructure activation is **native-only**. Browser uploads require
explicitly configured S3 CORS for the application origin, PUT, and the returned
signed headers; browser transport support alone does not enable S3 access.

```rust,ignore
let uploaded = client.clone().upload_image_file(
    "workers".into(),
    "rootfs.tar".into(),
    std::fs::read("rootfs.tar")?,
).await?;
assert!(uploaded.upload.is_none());
// Use uploaded.reference, uploaded.digest, and uploaded.size_bytes in the recipe.
```

The SDK hashes the bytes, calls the authenticated presign endpoint, and checks
that exactly one result binds the digest, size, and canonical tenant reference.
It reuses a matching existing object or PUTs the bytes to the HTTPS signed URL.
Returned headers are preserved, with checksum and length checked when supplied.
Presign requests have a 30-second timeout and a 1 MiB response limit. The PUT
has a 300-second timeout and a 4 KiB response limit, never acquires or refreshes
a token, and accepts only 200/201/204 as completed uploads. Both built-in
transports enforce deadlines across sending and reading the response body,
enforce response limits, and reject redirects; the helper never retries.
Failures redact signed URLs and response bodies while retaining error classes
and HTTP statuses. Successful
results contain no upload instructions.

This method does not create an Image. Only submit the recipe after every upload
succeeds. It does not attest to storage encryption, object versions, readiness,
or controller resolution; those remain backend/controller responsibilities.

Custom `HttpClient` implementations must obey the transport contract: no
redirects, retries, ambient authentication, or cookies; preserve supplied headers
and enforce response limits while streaming. The built-in native transport is
the preferred path. Browser uploads additionally require configured S3 CORS;
the browser manages Host and Content-Length itself, omits cookies, and rejects
redirects. The existing Node `FetchHttpClient`, Go `netHTTPClient`, and Swift
`UrlSessionHttpClient` examples use default redirect behavior (Swift also uses
shared session state); they are **not upload-safe adapters**. The Kotlin and Ruby
examples likewise do not enforce streaming response limits. None of these
illustrative custom adapters is certified for uploads: use the native constructor
instead or explicitly implement the full contract. This helper cannot override a
foreign implementation's HTTP policy.
