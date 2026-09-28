plugins { id("com.android.application") }
android {
    buildToolsVersion = "37.0.0"
    namespace = "ai.cua.fixture.notes"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig {
        applicationId = "ai.cua.fixture.notes"
        minSdk = 29
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
    }
}
dependencies { implementation(project(":sdk")) }
