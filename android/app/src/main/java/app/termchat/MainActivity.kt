package app.termchat

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import app.termchat.engine.*
import kotlin.concurrent.thread

/** A2 smoke screen: proves the Rust engine works on Android. The real UI is A3. */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // No screenshots, no preview in the recent apps list (ADR-011).
        window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        val dir = filesDir.absolutePath
        setContent { MaterialTheme(colorScheme = darkColorScheme()) { Surface { App(dir) } } }
    }
}

private class Ui : Listener {
    var version by mutableIntStateOf(0)
    var note by mutableStateOf("")
    override fun onEvent(event: Event) {
        when (event) {
            is Event.Note -> note = event.text
            is Event.Notice -> note = event.text
            is Event.Wiped -> note = "wiped " + event.note
            is Event.WipeAnyway -> note = event.why
            is Event.Alert -> {}
        }
    }
    override fun onChange() { version++ }
}

@Composable
private fun App(dir: String) {
    val ui = remember { Ui() }
    var account by remember { mutableStateOf<Account?>(null) }
    var error by remember { mutableStateOf("") }
    Column(Modifier.fillMaxSize().safeDrawingPadding().padding(12.dp)) {
        val a = account
        if (a == null) {
            Start(dir, ui) { result ->
                result.onSuccess { account = it; error = "" }.onFailure { error = it.message ?: "$it" }
            }
            if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
        } else {
            Chat(a, ui)
        }
    }
}

@Composable
private fun Start(dir: String, ui: Ui, done: (Result<Account>) -> Unit) {
    val exists = remember { stateExists(dir) }
    var relay by remember { mutableStateOf("wss://relay.termchat.app/") }
    var nick by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    if (!exists) {
        OutlinedTextField(relay, { relay = it }, label = { Text("Relay URL") })
        OutlinedTextField(nick, { nick = it }, label = { Text("Nickname") })
    }
    OutlinedTextField(
        password, { password = it }, label = { Text("Password") },
        visualTransformation = PasswordVisualTransformation(),
    )
    var busy by remember { mutableStateOf(false) }
    Button(enabled = !busy, onClick = {
        busy = true
        // Argon2 takes about a second: never on the main thread.
        thread {
            val result = runCatching {
                if (exists) unlock(dir, password, ui) ?: error("erased")
                else setup(dir, relay, nick, password, ui)
            }
            busy = false
            done(result)
        }
    }) { Text(if (busy) "…" else if (exists) "Unlock" else "Create") }
}

@Composable
private fun ColumnScope.Chat(a: Account, ui: Ui) {
    @Suppress("UNUSED_VARIABLE") val tick = ui.version
    val clipboard = LocalClipboardManager.current
    var open by remember { mutableStateOf<UInt?>(null) }
    var invite by remember { mutableStateOf("") }
    var text by remember { mutableStateOf("") }
    Text("${a.nickname()} · ${if (a.connected()) "online" else "offline"} · ${a.fingerprint().take(5)}")
    if (ui.note.isNotEmpty()) Text(ui.note, color = MaterialTheme.colorScheme.tertiary)
    Row {
        Button(onClick = { clipboard.setText(AnnotatedString(a.invite())) }) { Text("Copy my invite") }
    }
    Row {
        OutlinedTextField(invite, { invite = it }, label = { Text("Their invite") }, modifier = Modifier.weight(1f))
        Button(onClick = { runCatching { open = a.add(invite); invite = "" }.onFailure { ui.note = it.message ?: "" } }) { Text("Add") }
    }
    val contacts = a.contacts().sortedByDescending { it.lastAt }
    contacts.forEach { c ->
        Text(
            "${if (c.index == open) "▸ " else ""}${c.name}${if (c.unread > 0u) " (${c.unread})" else ""}${if (c.request) " · request" else ""}",
            Modifier.fillMaxWidth().clickable { open = c.index; a.open(c.index) }.padding(4.dp),
        )
    }
    val i = open ?: return
    val c = contacts.firstOrNull { it.index == i } ?: return
    if (c.request) Button(onClick = { a.accept(i) }) { Text("Accept") }
    LazyColumn(Modifier.weight(1f)) {
        items(a.messages(i)) { m ->
            Text(
                (if (m.mine) "you: " else if (m.system) "· " else "") + m.text + if (m.mine) "  [${m.status}]" else "",
                Modifier.padding(2.dp),
            )
        }
    }
    Row {
        OutlinedTextField(text, { text = it }, label = { Text("Encrypted message") }, modifier = Modifier.weight(1f))
        Button(onClick = { runCatching { a.send(i, text); text = "" }.onFailure { ui.note = it.message ?: "" } }) { Text("SEND") }
    }
}
