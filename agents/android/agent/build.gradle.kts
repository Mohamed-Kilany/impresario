// The agent app (docs/DECISIONS.md I-62): the instrumentation *target*, so
// the gRPC server and the UiAutomator driver run in this app's own process —
// never inside the app under test. The INTERNET permission belongs here (the
// server socket, reachable over the local network); the app under test needs
// none. Kotlin is compiled by AGP's built-in Kotlin support (AGP 9).
plugins {
    id("com.android.application")
}

android {
    namespace = "com.impresario.agent.app"
    compileSdk = 36

    defaultConfig {
        // Matches the Rust `AdbTransport` constant `AGENT_PACKAGE` and the
        // instrumentation's merged `android:targetPackage`.
        applicationId = "com.impresario.agent"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "1.0.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
