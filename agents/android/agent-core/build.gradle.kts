// `:agent-core` is pure Kotlin/JVM (no Android APIs): generated proto/gRPC-Kotlin
// stubs, gRPC server, session state, ordered dispatcher, error mapping,
// FocusNavigator and key tables. Everything that touches UiAutomator sits
// behind `UiDriver`, so the dispatcher is JVM-testable with a fake.
plugins {
    id("org.jetbrains.kotlin.jvm")
    id("com.google.protobuf")
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

// Single source of truth: the normative proto of the repository.
sourceSets {
    main {
        proto {
            srcDir("../../../proto")
        }
    }
}

val grpcJava = "1.85.0"
val grpcKotlin = "1.5.0"
val protobuf = "4.36.2"
val coroutines = "1.11.0"

protobuf {
    protoc {
        artifact = "com.google.protobuf:protoc:$protobuf"
    }
    plugins {
        create("grpc") {
            artifact = "io.grpc:protoc-gen-grpc-java:$grpcJava"
        }
        create("grpckt") {
            artifact = "io.grpc:protoc-gen-grpc-kotlin:$grpcKotlin:jdk8@jar"
        }
    }
    generateProtoTasks {
        all().configureEach {
            plugins {
                create("grpc")
                create("grpckt")
            }
        }
    }
}

dependencies {
    implementation("io.grpc:grpc-netty-shaded:$grpcJava")
    implementation("io.grpc:grpc-protobuf:$grpcJava")
    implementation("io.grpc:grpc-stub:$grpcJava")
    implementation("io.grpc:grpc-kotlin-stub:$grpcKotlin")
    implementation("com.google.protobuf:protobuf-java:$protobuf")
    implementation("com.google.protobuf:protobuf-kotlin:$protobuf")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:$coroutines")
    // Generated Java stubs reference @javax.annotation.Generated, which is not
    // in the JDK since 11.
    compileOnly("javax.annotation:javax.annotation-api:1.3.2")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:$coroutines")
}
