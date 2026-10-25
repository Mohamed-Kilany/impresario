pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "impresario-android"

include(":agent-core")
include(":driver-uiautomator")
include(":agent")
include(":agent-instrumentation")
include(":agent-target-stub")
include(":impresario-hook")
