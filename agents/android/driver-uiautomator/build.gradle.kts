// The UiAutomator-backed driver (spec 11.1, 11.3). Everything Android-specific
// lives here; `agent-core` stays plain Kotlin/JVM. Kotlin is compiled by AGP's
// built-in Kotlin support (AGP 9).
import org.gradle.api.attributes.LibraryElements

plugins {
    id("com.android.library")
}

android {
    namespace = "com.impresario.agent.driver"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // `api`: this module implements agent-core interfaces. The jar artifact is
    // requested explicitly so AGP dexes agent-core's Kotlin classes as well
    // (AGP 9.4.1 would otherwise take only `build/classes/java/main` of the
    // `classes` variant; see docs/DECISIONS.md I-56).
    api(project(":agent-core")) {
        attributes {
            attribute(
                LibraryElements.LIBRARY_ELEMENTS_ATTRIBUTE,
                objects.named(LibraryElements::class.java, LibraryElements.JAR),
            )
        }
    }
    implementation("androidx.test.uiautomator:uiautomator:2.4.0")
    // `SelectorStrategy` is a generated proto enum: its supertype
    // `com.google.protobuf.ProtocolMessageEnum` must be on this compile
    // classpath as well (agent-core keeps it `implementation`-scoped).
    implementation("com.google.protobuf:protobuf-java:4.36.2")
}
