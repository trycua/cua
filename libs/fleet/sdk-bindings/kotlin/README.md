# Kotlin Contract Binding

Requires JDK 21 and Gradle 8.10.2; Task 12 must install that exact Gradle release. All Kotlin/JNA/coroutines dependency versions are pinned in `build.gradle.kts`.

```bash
REPO_ROOT=/path/to/clone
export CYCLOPS_SDK_NATIVE_DIR="$(dirname "$($REPO_ROOT/cyclops-cs/scripts/build-sdk-bindings-native.sh)")"
gradle -p "$REPO_ROOT/cyclops-cs/sdk-bindings/kotlin" contract
# The example target is `gradle -p "$REPO_ROOT/cyclops-cs/sdk-bindings/kotlin" example`.
```

The project compiles generated schema and SDK sources together under the authoritative `ai.cua.cyclops.sdk` package and supplies JNA plus Kotlin coroutines required by UniFFI 0.32 output.
