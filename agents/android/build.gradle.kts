// AGP 9 has built-in Kotlin support, so Android modules do not apply the
// `org.jetbrains.kotlin.android` plugin (see docs/DECISIONS.md I-48). The pure
// JVM module `:agent-core` still uses `org.jetbrains.kotlin.jvm`.
plugins {
    id("com.android.application") version "9.4.1" apply false
    id("com.android.library") version "9.4.1" apply false
    id("com.android.test") version "9.4.1" apply false
    id("org.jetbrains.kotlin.jvm") version "2.4.21" apply false
    id("com.google.protobuf") version "0.9.6" apply false
}
