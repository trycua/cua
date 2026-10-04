// Compile check for the generated UniFFI Kotlin binding (no artifact is
// published yet). CI: `gradle -p libs/cua/kotlin compileKotlin` in the
// gradle:8-jdk17 container (ci-cua-sdk.yml, job `kotlin`).
rootProject.name = "cua-sdk-kotlin"
