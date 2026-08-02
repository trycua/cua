plugins { kotlin("jvm") version "2.1.0" }
dependencies {
    implementation("net.java.dev.jna:jna:5.16.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
}
kotlin { jvmToolchain(21) }
sourceSets.main { kotlin.srcDirs("ai") }
sourceSets.test { kotlin.srcDirs("tests") }
val example by sourceSets.creating {
    kotlin.srcDir("../examples/kotlin")
    compileClasspath += sourceSets.main.get().output
    runtimeClasspath += sourceSets.main.get().output
}
configurations[example.implementationConfigurationName].extendsFrom(
    configurations[sourceSets.main.get().implementationConfigurationName],
)
configurations[example.runtimeOnlyConfigurationName].extendsFrom(
    configurations[sourceSets.main.get().runtimeOnlyConfigurationName],
)
tasks.register<JavaExec>("contract") {
    classpath = sourceSets.test.get().runtimeClasspath
    mainClass.set("TestAsyncClientKt")
    jvmArgs("-Djna.library.path=" + System.getenv("CYCLOPS_SDK_NATIVE_DIR"))
}
tasks.register<JavaExec>("example") {
    dependsOn(example.classesTaskName)
    classpath = example.runtimeClasspath
    mainClass.set("AppControlledKt")
    jvmArgs("-Djna.library.path=" + System.getenv("CYCLOPS_SDK_NATIVE_DIR"))
}
tasks.register<JavaExec>("liveExample") {
    dependsOn(example.classesTaskName)
    classpath = example.runtimeClasspath
    mainClass.set("LiveAppControlledKt")
    jvmArgs("-Djna.library.path=" + System.getenv("CYCLOPS_SDK_NATIVE_DIR"))
}
