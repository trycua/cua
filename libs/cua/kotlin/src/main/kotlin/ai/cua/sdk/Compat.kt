package ai.cua.sdk

// Sandbox refs: an ambiguous bare name lists the qualified refs it matches.

/** The link to this error's entry (cause and fix) on the errors reference. */
val CuaException.docUrl: String
    get() = errorDocUrl(this::class.java.simpleName)

/** The qualified refs (`local:box`, `cloud:box`) this ambiguity lists. */
val CuaException.AmbiguousSandbox.candidates: List<String>
    get() = ambiguousSandboxCandidates(message ?: "")
