plugins { id("com.android.application") }
android {
    buildToolsVersion = "37.0.0"
    namespace = "ai.cua.driver"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig {
        applicationId = "ai.cua.driver.runtime"
        minSdk = 37
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0-experimental"
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
dependencies { implementation(project(":sdk")) }
