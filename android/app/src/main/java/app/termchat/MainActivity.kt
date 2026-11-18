package app.termchat

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
        setContent { TermchatTheme { NoLearning { Surface(Modifier.fillMaxSize()) { Root(model) } } } }
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
            is Screen.Chat -> Chat(m, s.index)
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
                        Text("Both screens show this fingerprint. Go on only if they are the same: someone who copied the link code would show another one.", style = Tc.mono)
                        Text(a.linkFingerprint(), style = Tc.name.copy(color = Tc.lime))
                    }
                },
                confirmButton = { Flat("The same") { a.confirmLink(i); m.note = "confirmed: linking finishes when the other screen agrees"; m.changed() } },
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
