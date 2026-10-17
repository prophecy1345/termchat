plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The release key stays outside the repository (ADR-011): a local path or CI
// secrets, through the environment. F-Droid strips signing blocks and signs
// nothing, so the config is made in one expression, only when a key is given.
fun releaseKey(project: Project, path: String) =
    (project.extensions.getByName("android") as com.android.build.api.dsl.ApplicationExtension)
        .signingConfigs.create("release") {
            storeFile = project.file(path)
            storePassword = System.getenv("TERMCHAT_KEYSTORE_PASSWORD")
            keyAlias = "termchat"
            keyPassword = System.getenv("TERMCHAT_KEYSTORE_PASSWORD")
        }

android {
    namespace = "app.termchat"
    compileSdk = 37
    defaultConfig {
        applicationId = "app.termchat"
        minSdk = 26
        targetSdk = 37
        // Written out for F-Droid's update checker, which reads them here;
        // checked against the workspace's Cargo.toml at the end of this file.
        versionName = "0.2.2"
        versionCode = 202
    }
    buildFeatures { compose = true }
    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = System.getenv("TERMCHAT_KEYSTORE")?.let { releaseKey(project, it) }
        }
    }
    // One APK per ABI: phones need only arm64; x86_64 is for the emulator.
    splits {
        abi {
            isEnable = true
            reset()
            include("arm64-v8a", "x86_64")
            isUniversalApk = false
        }
    }
    // Reproducible: no build timestamps or VCS info in the APK.
    dependenciesInfo { includeInApk = false; includeInBundle = false }
    // Built by ../build-rust.sh: libtermchat_ffi.so and its Kotlin bindings.
    sourceSets["main"].jniLibs.directories.add("build/rust/jniLibs")
    sourceSets["main"].kotlin.directories.add("build/rust/kotlin")
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.13.0")
    // UniFFI's Kotlin bindings call the library through JNA.
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    // Drawing and scanning invite QR codes, without Google ML Kit (ADR-011).
    implementation("com.google.zxing:core:3.5.4")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0") { isTransitive = false }
    // Wake-ups through the push server the user picked, without Google (ADR-012).
    implementation("org.unifiedpush.android:connector:3.3.5")
}

// One version for the whole project: Cargo.toml. versionCode is
// major * 10000 + minor * 100 + patch (CONTRIBUTING.md, "Versions").
val cargoVersion = rootDir.resolve("../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }.substringAfter('"').substringBefore('"')
val (major, minor, patch) = cargoVersion.split('.').map { it.toInt() }
check(
    android.defaultConfig.versionName == cargoVersion &&
        android.defaultConfig.versionCode == major * 10000 + minor * 100 + patch,
) { "android/app/build.gradle.kts disagrees with Cargo.toml ($cargoVersion)" }
