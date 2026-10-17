plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// One version for the whole project: the workspace's Cargo.toml.
val cargoVersion = rootDir.resolve("../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }.substringAfter('"').substringBefore('"')
val (major, minor, patch) = cargoVersion.split('.').map { it.toInt() }

android {
    namespace = "app.termchat"
    compileSdk = 37
    defaultConfig {
        applicationId = "app.termchat"
        minSdk = 26
        targetSdk = 37
        versionName = cargoVersion
        versionCode = major * 10000 + minor * 100 + patch
    }
    buildFeatures { compose = true }
    // The release key stays outside the repository (ADR-011): a local path or
    // CI secrets, through the environment.
    signingConfigs {
        create("release") {
            storeFile = System.getenv("TERMCHAT_KEYSTORE")?.let { file(it) }
            storePassword = System.getenv("TERMCHAT_KEYSTORE_PASSWORD")
            keyAlias = "termchat"
            keyPassword = System.getenv("TERMCHAT_KEYSTORE_PASSWORD")
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs["release"].takeIf { it.storeFile != null }
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
