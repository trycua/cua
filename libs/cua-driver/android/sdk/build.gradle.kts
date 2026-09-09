plugins { id("com.android.library") }
android {
    buildToolsVersion = "37.0.0"
    namespace = "ai.cua.driver.sdk"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig { minSdk = 29 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
