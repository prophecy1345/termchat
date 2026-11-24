package app.termchat

import android.content.Intent
import android.content.pm.ApplicationInfo
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.input.pointer.pointerInput
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {
    private lateinit var model: Model

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // No screenshots, no preview in the recent apps list (ADR-011). Debug
        // builds, never shipped, allow them so the UI can be checked.
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0) {
            window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        }
        model = (application as TermchatApp).model
        takeShare(intent)
        setContent { TermchatTheme { NoLearning { Surface(Modifier.fillMaxSize()) { Root(model) } } } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        takeShare(intent)
    }

    /** Text or one file from another app's share sheet; nothing is sent until a chat is picked. */
    private fun takeShare(intent: Intent?) {
        if (intent?.action != Intent.ACTION_SEND) return
        val text = intent.getStringExtra(Intent.EXTRA_TEXT)?.takeIf { it.isNotBlank() }
        @Suppress("DEPRECATION")
        val uri = intent.getParcelableExtra<android.net.Uri>(Intent.EXTRA_STREAM)
        model.share = when {
            uri != null -> runCatching { readDocument(this, uri) }
                .map { (name, bytes) -> Share(null, name, bytes) }
                .getOrElse { model.note = it.message ?: "this file cannot be shared"; null }
            text != null -> Share(text, null, null)
            else -> null
        }
        // Handled once: a rotation must not bring the same share back.
        intent.action = null
    }

    override fun onStop() {
        super.onStop()
        model.visible = false
        model.armLock()
    }

    override fun onStart() {
        super.onStart()
        LockAlarm.disarm(this)
        // Before anything shows or a touch can reset the time: the periodic
        // check stops while the phone sleeps. Stay connected: time in the
        // background does not count (ADR-012); otherwise it does (ADR-011).
        if (model.stay) model.touch() else model.lockIfIdle()
        model.visible = true
        model.account?.reconnect()
        model.seen()
    }
}

@Composable
private fun Root(m: Model) {
    BackHandler(enabled = m.screen != Screen.Chats && m.screen != Screen.Start) { m.screen = Screen.Chats }
    Box(
        Modifier.fillMaxSize().safeDrawingPadding().imePadding().pointerInput(Unit) {
            awaitPointerEventScope { while (true) { awaitPointerEvent(); m.touch() } }
        },
    ) {
        when (val s = m.screen) {
            Screen.Start -> Start(m)
            Screen.Chats -> Chats(m)
            is Screen.Chat -> Chat(m, s.index, s.ask)
            is Screen.Group -> Group(m, s.index)
            Screen.Invite -> Invite(m)
            Screen.Add -> Add(m)
            Screen.Settings -> Settings(m)
            Screen.News -> News(m)
            Screen.Bridges -> Bridges(m)
            Screen.Devices -> Devices(m)
            Screen.LinkCode -> LinkCode(m)
        }
    }
    // Shared from another app: pick where it goes once the identity is open.
    if (m.share != null && m.account != null && m.screen != Screen.Start) ShareTo(m)
    // A note belongs to the screen it was said on: leaving it ends the note,
    // so one from a chat does not linger over the list.
    var noteScreen by remember { mutableStateOf(m.screen) }
    if (noteScreen != m.screen) {
        noteScreen = m.screen
        m.note = ""
    }
    // A note is news, not a state: it goes after a while.
    LaunchedEffect(m.note) {
        if (m.note.isNotEmpty()) {
            delay(6000)
            m.note = ""
        }
    }
    // ADR-028: a device waiting for the fingerprint check asks over any screen.
    @Suppress("UNUSED_EXPRESSION") m.version
    m.account?.let { a ->
        a.pendingLink()?.let { i ->
            AlertDialog(
                onDismissRequest = {},
                containerColor = Tc.surface,
                shape = corner,
                title = { Text("Link a device", style = Tc.name) },
                text = {
                    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        if (a.linkWaiting(i)) {
                            // Our answer is still on its way: the other screen has nothing to compare yet.
                            Text("Reaching the new device… Its screen shows the fingerprint in a moment (through Tor this takes up to half a minute). Then compare.", style = Tc.mono)
                        } else {
                            Text("Both screens show this fingerprint. Go on only if they are the same: someone who copied the link code would show another one.", style = Tc.mono)
                            Text(a.linkFingerprint(), style = Tc.name.copy(color = Tc.lime))
                        }
                    }
                },
                confirmButton = { Flat("The same", enabled = !a.linkWaiting(i)) { a.confirmLink(i); m.note = "confirmed: linking finishes when the other screen agrees"; m.changed() } },
                dismissButton = { Flat("Cancel", color = Tc.dim) { a.cancelLink(i); m.note = "linking cancelled"; m.changed() } },
            )
        }
    }
    m.notice?.let { Dialog(it, "OK") { m.notice = null } }
    m.wipeAnyway?.let {
        AlertDialog(
            onDismissRequest = { m.wipeAnyway = null },
            containerColor = Tc.surface,
        shape = corner,
            title = { Text("Wipe without telling your contacts?", style = Tc.name) },
            text = { Text(it, style = Tc.mono) },
            confirmButton = { Flat("Wipe anyway", color = Tc.danger) { m.wipeAnyway = null; m.account?.wipeNow() } },
            dismissButton = { Flat("Cancel", color = Tc.dim) { m.wipeAnyway = null } },
        )
    }
    if (m.leaving) Dialog("Telling your contacts and the relay…", null) {}
}

@Composable
private fun Dialog(text: String, ok: String?, close: () -> Unit) = AlertDialog(
    onDismissRequest = { if (ok != null) close() },
    containerColor = Tc.surface,
        shape = corner,
    text = { Text(text, style = Tc.mono) },
    confirmButton = { if (ok != null) Flat(ok, onClick = close) },
)
