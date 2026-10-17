package app.termchat

import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.DocumentsContract
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import app.termchat.engine.Account
import app.termchat.engine.Event
import app.termchat.engine.Listener
import java.io.File
import kotlin.concurrent.thread

/** Idle time before the app locks itself, as in the terminal client. */
const val AUTOLOCK_MS = 5 * 60 * 1000L

sealed interface Screen {
    data object Start : Screen
    data object Chats : Screen
    data class Chat(val index: UInt) : Screen
    data object Invite : Screen
    data object Add : Screen
    data object Settings : Screen
    data object News : Screen
}

/** Everything on screen; the engine reports through [Listener] on its own thread. */
class Model(private val context: Context) : Listener {
    val dir: String = context.filesDir.absolutePath
    private val main = Handler(Looper.getMainLooper())

    var account by mutableStateOf<Account?>(null)
        private set
    var screen by mutableStateOf<Screen>(Screen.Start)
    /** Bumped on every change, so screens re-read the account. */
    var version by mutableIntStateOf(0)
        private set
    var note by mutableStateOf("")
    var notice by mutableStateOf<String?>(null)
    var wipeAnyway by mutableStateOf<String?>(null)
    var leaving by mutableStateOf(false)
    /** Shown on the first-run screen after a wipe. */
    var wipedNote by mutableStateOf("")
    /** Counts wipes, so the first-run screen looks at the disk again. */
    var wipes by mutableIntStateOf(0)
        private set
    var busy by mutableStateOf(false)
    var lastInput = System.currentTimeMillis()
    /** The app is on screen (between onStart and onStop). */
    var visible = false

    // Plain settings, like the terminal's settings.json (ADR-008); wiped with the state.
    private val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
    var notify by mutableStateOf(prefs.getBoolean("notify", false))
        private set
    /** Keep the relay connection while the app is in the background (ADR-012). */
    var stay by mutableStateOf(prefs.getBoolean("stay", false))
        private set

    /** The UnifiedPush endpoint the relay should wake (ADR-012); kept for the next unlock. */
    var push by mutableStateOf(prefs.getString("push", null))
        private set

    fun pushEndpoint(url: String?) {
        push = url
        prefs.edit().putString("push", url).apply()
        account?.setPush(url)
    }

    /** A wake-up from the push server: a message waits on the relay. */
    fun woken() {
        if (account == null || !visible) {
            context.getSystemService(android.app.NotificationManager::class.java)
                .notify(ID_MESSAGE, notification(context, CHANNEL_MESSAGES, "New message"))
        }
    }

    fun notifyOn(on: Boolean) { notify = on; prefs.edit().putBoolean("notify", on).apply() }

    fun stayOn(on: Boolean) {
        stay = on
        prefs.edit().putBoolean("stay", on).apply()
        link()
    }

    /** The service runs while an account is open and the user asked to stay connected. */
    private fun link() {
        if (account != null && stay) ConnectionService.start(context) else ConnectionService.stop(context)
    }

    // Runs as long as the process does, with or without a screen.
    private val idleCheck = object : Runnable {
        override fun run() { lockIfIdle(); main.postDelayed(this, 10_000) }
    }

    init {
        channels(context)
        main.postDelayed(idleCheck, 10_000)
    }

    override fun onEvent(event: Event) = main.post {
        when (event) {
            is Event.Note -> note = event.text
            is Event.Notice -> notice = event.text
            is Event.WipeAnyway -> { leaving = false; wipeAnyway = event.why }
            is Event.Wiped -> wiped(event.note)
            is Event.Alert -> alert(event.index)
        }
    }.let {}

    override fun onChange() = main.post { version++ }.let {}

    /** Runs [work] off the main thread (Argon2 takes about a second), then [done] on it. */
    fun <T> slow(work: () -> T, done: (Result<T>) -> Unit) {
        busy = true
        thread {
            val result = runCatching(work)
            main.post { busy = false; done(result) }
        }
    }

    fun opened(a: Account) {
        account = a
        if (a.push() != push) a.setPush(push)
        lastInput = System.currentTimeMillis()
        screen = Screen.Chats
        link()
    }

    /** "New message", nothing else, and only when the chat is not in front of the user. */
    private fun alert(index: UInt?) {
        val open = (screen as? Screen.Chat)?.index
        if (!notify || (visible && open != null && open == index)) return
        context.getSystemService(android.app.NotificationManager::class.java)
            .notify(ID_MESSAGE, notification(context, CHANNEL_MESSAGES, "New message"))
    }

    /** Saves and drops every key and message from memory. */
    fun lock() {
        account?.destroy()
        account = null
        screen = Screen.Start
        notice = null
        leaving = false
        link()
    }

    fun touch() { lastInput = System.currentTimeMillis() }

    /** After the user changed something: the engine only reports the relay's changes. */
    fun changed() { version++ }

    /** Five minutes without a touch lock it, on screen or in the background (ADR-011),
     *  unless it is in the background and the user asked to stay connected (ADR-012). */
    fun lockIfIdle() {
        if (account != null && (visible || !stay) && System.currentTimeMillis() - lastInput >= AUTOLOCK_MS) lock()
    }

    /** The state file is gone: SAF backups we know of go too (ADR-003, ADR-011). */
    fun wiped(note: String) {
        eraseBackups()
        account?.destroy()
        account = null
        leaving = false
        wipeAnyway = null
        wipedNote = note
        wipes++
        screen = Screen.Start
        prefs.edit().clear().apply()
        notify = false
        stay = false
        if (push != null) org.unifiedpush.android.connector.UnifiedPush.unregister(context)
        push = null
        link()
    }

    // Backups written or restored through the Storage Access Framework. The
    // engine erases files by path; a content URI can only be deleted, and only
    // from here. Kept beside the state, plain, like the engine's own list.
    private val safList get() = File(dir, "backups-saf")

    fun rememberBackup(uri: Uri) {
        val known = safList.takeIf { it.exists() }?.readLines().orEmpty()
        if (uri.toString() !in known) safList.appendText(uri.toString() + "\n")
    }

    private fun eraseBackups() {
        val list = safList.takeIf { it.exists() } ?: return
        for (line in list.readLines()) {
            runCatching { DocumentsContract.deleteDocument(context.contentResolver, Uri.parse(line)) }
        }
        list.delete()
    }
}
