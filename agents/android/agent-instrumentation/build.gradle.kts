// The instrumentation APK (spec 11.1): `AgentRunner` is the class
// `AdbTransport` starts with `am instrument`; the instrumented tests validate
// the driver and the in-process gRPC server on the device. It targets the
// `:agent` app, so the agent's process is the agent app's own process — the
// app under test is driven cross-app and killing it never kills the agent
// (docs/DECISIONS.md I-62).
// Kotlin is compiled by AGP's built-in Kotlin support (AGP 9).
import org.gradle.api.attributes.LibraryElements

plugins {
    id("com.android.test")
}

android {
    // The test APK package must match `AGENT_INSTRUMENTATION_PACKAGE` in the
    // Rust `AdbTransport` (`com.impresario.agent.test`); AGP would otherwise use
    // the module namespace verbatim. The `android:targetPackage` AGP merges is
    // the agent app's applicationId (`com.impresario.agent`), so `am instrument`
    // attaches to the agent process (docs/DECISIONS.md I-49).
    namespace = "com.impresario.agent.test"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
        targetSdk = 36
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // Overridable with `-e port <n>`; spec 11.2 default.
        testInstrumentationRunnerArguments["port"] = "50061"
    }

    targetProjectPath = ":agent"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // The jar artifact is requested explicitly: AGP 9.4.1 resolves a
    // `org.jetbrains.kotlin.jvm` project through the `classes` secondary
    // variant and dexes only its first artifact (`build/classes/java/main`),
    // silently dropping the Kotlin output (`build/classes/kotlin/main`). The
    // jar holds both (docs/DECISIONS.md I-56).
    implementation(project(":agent-core")) {
        attributes {
            attribute(
                LibraryElements.LIBRARY_ELEMENTS_ATTRIBUTE,
                objects.named(LibraryElements::class.java, LibraryElements.JAR),
            )
        }
    }
    implementation(project(":driver-uiautomator"))

    implementation("androidx.test:runner:1.7.0")
    implementation("junit:junit:4.13.2")

    // In-process gRPC client for AgentServerInstrumentedTest.
    implementation("io.grpc:grpc-netty-shaded:1.85.0")
    implementation("io.grpc:grpc-protobuf:1.85.0")
    implementation("io.grpc:grpc-stub:1.85.0")
    implementation("io.grpc:grpc-kotlin-stub:1.5.0")
    implementation("com.google.protobuf:protobuf-java:4.36.2")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0")
}
