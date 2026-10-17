package app.termchat

import android.app.Application

/** Holds the model for the whole process, so the connection service can keep it. */
class TermchatApp : Application() {
    lateinit var model: Model
        private set

    override fun onCreate() {
        super.onCreate()
        model = Model(this)
    }
}
