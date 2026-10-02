package ai.cua.sdk

/**
 * `Pool.apply(fleet, name, spec, options)`: the one pool writer, the same
 * call as `fleet.apply`. A [SandboxSpec] is what the sandbox runs and
 * [PoolOptions] how the pool keeps capacity for it. `fleet.checkPoolSpec`
 * throws `CuaException.PoolSpecMismatch` (with a diff) for a named pool
 * whose template differs, `fleet.applyPoolTemplate` updates it, and
 * `fleet.exportPool(name).terraform` prints the `fleets_pool` block.
 *
 * ```kotlin
 * val spec = SandboxSpec(image = "python:3.12-slim", command = listOf("python", "-m", "srv"),
 *     services = mapOf("mcp" to 8765.toUShort()))
 * Pool.apply(cua.fleet(), "my-pool", spec, PoolOptions(warm = true, idleTtlSeconds = 3600u))
 * ```
 */
object Pool {
    suspend fun apply(
        fleet: FleetInterface,
        name: String,
        spec: SandboxSpec,
        options: PoolOptions = PoolOptions(),
    ): FleetPool = fleet.apply(name, spec, options)

    /** A spec that only sets the fields given (the rest keep Fleet's defaults). */
    fun spec(
        image: String,
        command: List<String>? = null,
        env: Map<String, String> = emptyMap(),
        services: Map<String, Int> = emptyMap(),
        cpu: Int? = null,
        memoryMb: Int? = null,
        claimSecrets: Boolean = false,
    ): SandboxSpec = SandboxSpec(
        image = image,
        command = command,
        env = env,
        services = services.mapValues { it.value.toUShort() },
        cpu = cpu?.toUInt(),
        memoryMb = memoryMb?.toUInt(),
        claimSecrets = claimSecrets,
    )
}
