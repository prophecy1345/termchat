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
    }

    override fun onStart() {
        super.onStart()
        model.visible = true
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
