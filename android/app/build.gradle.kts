plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "app.termchat"
    compileSdk = 37
    defaultConfig {
        applicationId = "app.termchat"
        minSdk = 26
        targetSdk = 37
        versionName = "0.1.12"
        versionCode = 12
    }
    buildFeatures { compose = true }
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
}
