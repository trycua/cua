plugins { id("com.android.application") }
android {
    buildToolsVersion = "37.0.0"
    namespace = "ai.cua.android.demo"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig {
        applicationId = "ai.cua.android.demo"
        minSdk = 37
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
    }
}
dependencies {
    implementation(project(":sdk"))
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}
