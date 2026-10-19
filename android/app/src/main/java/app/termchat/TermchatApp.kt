package app.termchat

import android.app.Application

/** Holds the model for the whole process, so the connection service can keep it. */
class TermchatApp : Application() {
    lateinit var model: Model
        private set

    override fun onCreate() {
        super.onCreate()
        // Tor bridges (ADR-015): lyrebird ships as a native library, the only
        // place Android lets an app start its own program from.
        app.termchat.engine.setTransportProgram("${applicationInfo.nativeLibraryDir}/liblyrebird.so")
        model = Model(this)
    }
}
