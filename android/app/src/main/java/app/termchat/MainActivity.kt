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
import androidx.compose.ui.input.pointer.pointerInput
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {
    private lateinit var model: Model
    private var hiddenSince = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // No screenshots, no preview in the recent apps list (ADR-011). Debug
        // builds, never shipped, allow them so the UI can be checked.
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0) {
            window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        }
        model = Model(applicationContext)
        setContent { TermchatTheme { NoLearning { Surface(Modifier.fillMaxSize()) { Root(model) } } } }
    }

    // A recreated activity starts locked; the old account must not live on in
    // the background with its keys and its relay connection.
    override fun onDestroy() {
        model.lock()
        super.onDestroy()
    }

    override fun onStop() {
        super.onStop()
        hiddenSince = System.currentTimeMillis()
    }

    override fun onStart() {
        super.onStart()
        // Five minutes in the background lock it, like five idle minutes (ADR-011).
        if (hiddenSince > 0 && System.currentTimeMillis() - hiddenSince >= AUTOLOCK_MS) model.lock()
    }
}

@Composable
private fun Root(m: Model) {
    LaunchedEffect(Unit) {
        while (true) { delay(10_000); m.lockIfIdle() }
    }
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
            Screen.Invite -> Invite(m)
            Screen.Add -> Add(m)
            Screen.Settings -> Settings(m)
            Screen.News -> News(m)
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
