package ai.cua.sdk

/**
 * Canonical images: `Image.linux()` is `ghcr.io/trycua/linux:24.04`
 * (`CUA_IMAGE_LINUX` overrides it), `Image.windows()`
 * `ghcr.io/trycua/windows:2022`, `Image.macos()` `ghcr.io/trycua/macos:26`.
 * These are the full tier (dev tooling); `tier = "slim"` is the minimal image
 * CI runs and, on macOS, `tier = "xcode"` adds a pinned Xcode.
 * `Image.omarchy()` is `ghcr.io/trycua/omarchy:edge`, an amd64 VM. Images CI
 * has not published yet throw `CuaException.ImageNotPublished`; pass the
 * reference to `fromRegistry` to use one anyway.
 * `Image.resolve` is the one resolver (digest-pinned variant per backend).
 */
object Image {
    fun linux(version: String? = null, tier: String? = null): String =
        canonicalImageTier("linux", version, tier)

    fun windows(version: String? = null, tier: String? = null): String =
        canonicalImageTier("windows", version, tier)

    fun macos(version: String? = null, tier: String? = null): String =
        canonicalImageTier("macos", version, tier)

    /** Omarchy (Arch Linux, Hyprland) with cua-spacesd: an amd64 VM. */
    fun omarchy(channel: String? = null): String = omarchyImage(channel)

    // Literal: `ubuntu:24.04` is Docker Hub's image.
    fun fromRegistry(reference: String): String = reference

    fun resolve(reference: String, backend: String = "local", arch: String? = null): ResolvedImage =
        resolveImage(reference, backend, arch)
}
