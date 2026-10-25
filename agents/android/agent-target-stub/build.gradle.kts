// The stub app under test (spec 11.1): a real, separate app the agent drives
// cross-app — never the agent's own process. Its home screen lists one
// scenario per test; every scenario has its own screen, all D-pad navigable
// so the same APK is the phone and the TV app under test. It declares no
// permissions (docs/DECISIONS.md I-62).
// Kotlin is compiled by AGP's built-in Kotlin support (AGP 9).
plugins {
    id("com.android.application")
}

android {
    namespace = "com.impresario.stub"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.impresario.stub"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(project(":impresario-hook"))
}
