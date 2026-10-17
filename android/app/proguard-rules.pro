# UniFFI's Kotlin calls into libtermchat_ffi.so through JNA, by name.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-dontwarn java.awt.**
-keep class app.termchat.engine.** { *; }
