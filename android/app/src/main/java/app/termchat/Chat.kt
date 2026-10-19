package app.termchat

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import app.termchat.engine.ContactView
import app.termchat.engine.MessageView

/** One chat (ADR-011, section 5). */
@Composable
fun Chat(m: Model, index: UInt) {
    val a = m.account ?: return
    @Suppress("UNUSED_EXPRESSION") m.version
    val c = a.contacts().firstOrNull { it.index == index } ?: return run { m.screen = Screen.Chats }
    val messages = runCatching { a.messages(index) }.getOrDefault(emptyList())
    var text by remember { mutableStateOf("") }
    var dialog by remember { mutableStateOf<String?>(null) }
    // Search runs here, over what is already on the phone; `null` is off.
    var query by remember(index) { mutableStateOf<String?>(null) }
    DisposableEffect(index) {
        a.open(index)
        onDispose { a.open(null) }
    }
    LaunchedEffect(m.version) { a.open(index) }

    Column(Modifier.fillMaxSize()) {
        Header(m, c) { if (it == "search") query = "" else dialog = it }
        Line()
        query?.let { q ->
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Field(q, { query = it }, "Search this chat", Modifier.weight(1f))
                Text("×", style = Tc.name, modifier = Modifier.clickable { query = null }.padding(12.dp))
            }
        }
        val shown = query?.takeIf { it.isNotBlank() }?.let { q ->
            messages.filter { !it.system && it.text.contains(q.trim(), ignoreCase = true) }
        } ?: messages
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
            reverseLayout = true,
            verticalArrangement = Arrangement.spacedBy(10.dp),
            contentPadding = PaddingValues(vertical = 12.dp),
        ) {
            items(shown.asReversed()) { Bubble(it) }
        }
        if (c.request) {
            Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Flat("Accept") { runCatching { a.accept(index) }; m.changed() }
                Flat("Reject", color = Tc.danger) { dialog = "delete" }
            }
        }
        if (m.note.isNotEmpty()) Text(m.note, style = Tc.small.copy(color = Tc.warn), modifier = Modifier.padding(horizontal = 12.dp))
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            // The drop cycles the disappearing timer, like Ctrl+T.
            Column(
                Modifier.size(48.dp).border(1.dp, Tc.line, corner).clickable { runCatching { a.nextTimer(index) }; m.changed() },
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                Drop(if (c.expiresIn != null) Tc.lime else Tc.dim)
                if (c.expiresIn != null) Text(span(c.expiresIn), style = Tc.small.copy(color = Tc.lime))
            }
            val closed = c.left || c.blocked
            Field(
                text, { text = it },
                when { c.left -> "This contact left"; c.blocked -> "Blocked"; else -> "Encrypted message" },
                Modifier.weight(1f), singleLine = false,
            )
            Flat("Send", enabled = text.isNotBlank() && !closed, color = Tc.lime) {
                runCatching { a.send(index, text) }
                    .onSuccess { text = ""; m.note = ""; m.changed() }
                    .onFailure { m.note = it.message ?: "$it" }
            }
        }
    }
    dialog?.let { ContactDialog(m, c, it) { dialog = null } }
}

@Composable
private fun Header(m: Model, c: ContactView, ask: (String) -> Unit) {
    val a = m.account ?: return
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("‹", style = Tc.name, modifier = Modifier.clickable { m.screen = Screen.Chats }.padding(end = 12.dp))
        Avatar(c.name)
        Column(Modifier.weight(1f).padding(start = 12.dp)) {
            Text(c.name, style = Tc.name)
            val (state, ok) = when {
                c.left -> "LEFT" to false
                c.blocked -> "BLOCKED" to false
                c.verified -> "ENCRYPTED · VERIFIED" to true
                else -> "ENCRYPTED · NOT VERIFIED" to false
            }
            Text(state, style = Tc.caps.copy(color = if (ok) Tc.lime else Tc.warn))
            c.incognito?.let { Text("INCOGNITO AS ${it.uppercase()}", style = Tc.caps.copy(color = Tc.dim)) }
        }
        Box {
            Text("⋮", style = Tc.name, modifier = Modifier.clickable { menu = true }.padding(8.dp))
            DropdownMenu(menu, { menu = false }) {
                val item = @Composable { label: String, go: () -> Unit ->
                    DropdownMenuItem({ Text(label, style = Tc.mono) }, { menu = false; go() })
                }
                item("Search") { ask("search") }
                item("Safety number") { ask("verify") }
                item("Rename") { ask("rename") }
                item("Disappearing: ${span(c.expiresIn)}") { runCatching { a.nextTimer(c.index) }; m.changed() }
                item("No history: ${if (c.noHistory) "on" else "off"}") { runCatching { a.setNoHistory(c.index, !c.noHistory) }; m.changed() }
                item(if (c.blocked) "Unblock" else "Block") { runCatching { a.setBlocked(c.index, !c.blocked) }; m.changed() }
                item("Report to the relay operator") { ask("report") }
                item("Delete") { ask("delete") }
            }
        }
    }
}

@Composable
private fun Bubble(msg: MessageView) {
    if (msg.system) {
        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
            Text(
                msg.text,
                style = Tc.small,
                modifier = Modifier.border(1.dp, Tc.line, corner).padding(horizontal = 12.dp, vertical = 6.dp),
            )
        }
        return
    }
    Column(Modifier.fillMaxWidth(), horizontalAlignment = if (msg.mine) Alignment.End else Alignment.Start) {
        SelectionContainer {
            Text(
                msg.text,
                style = Tc.mono,
                modifier = Modifier
                    .widthIn(max = 300.dp)
                    .background(if (msg.mine) Tc.mine else Tc.theirs, corner)
                    .padding(horizontal = 14.dp, vertical = 10.dp),
            )
        }
        val failed = msg.mine && msg.status !in setOf("sent", "sending", "queued")
        val meta = when {
            failed -> "failed: ${msg.status}"
            msg.mine && msg.status != "sent" -> msg.status
            else -> age(msg.at)
        }
        Text(meta, style = Tc.small.copy(color = if (failed) Tc.danger else Tc.dim), modifier = Modifier.padding(top = 3.dp, start = 2.dp, end = 2.dp))
    }
}

/** The drop icon of the timer button. */
@Composable
private fun Drop(color: androidx.compose.ui.graphics.Color) {
    Box(Modifier.size(width = 14.dp, height = 18.dp).drawBehind {
        val w = size.width
        val h = size.height
        val p = Path().apply {
            moveTo(w / 2, 0f)
            cubicTo(w * 0.9f, h * 0.45f, w, h * 0.6f, w, h * 0.7f)
            cubicTo(w, h * 0.9f, w * 0.75f, h, w / 2, h)
            cubicTo(w * 0.25f, h, 0f, h * 0.9f, 0f, h * 0.7f)
            cubicTo(0f, h * 0.6f, w * 0.1f, h * 0.45f, w / 2, 0f)
            close()
        }
        drawPath(p, color, style = Stroke(width = 1.5.dp.toPx()))
        drawCircle(color, radius = 0.6.dp.toPx(), center = Offset(w / 2, h * 0.7f))
    })
}

@Composable
private fun ContactDialog(m: Model, c: ContactView, which: String, close: () -> Unit) {
    val a = m.account ?: return
    var input by remember { mutableStateOf("") }
    // ADR-006: exactly the text shown is what goes to the operator, even if
    // more arrives while the dialog is open.
    val reportText = remember { if (which == "report") runCatching { a.reportText(c.index) }.getOrNull() else null }
    val (title, body, confirm) = when (which) {
        "verify" -> Triple(
            "Safety number",
            "Compare it with ${c.name} in person or on a call. If it matches, mark verified.\n\n" +
                runCatching { a.safetyNumber(c.index) }.getOrDefault(""),
            if (c.verified) "Mark not verified" else "Mark verified",
        )
        "rename" -> Triple("Rename", "Only you see this name.", "Save")
        "report" -> Triple(
            "Report",
            reportText?.let {
                "The relay operator gets exactly this text, and the contact is blocked:\n\n$it"
            } ?: "Nothing from them to report.",
            "Report and block",
        )
        else -> Triple("Delete ${c.name}?", "The chat and the contact are removed from this phone.", "Delete")
    }
    AlertDialog(
        onDismissRequest = close,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text(title, style = Tc.name) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(body, style = Tc.mono)
                if (which == "rename") Field(input, { input = it }, c.name, Modifier.fillMaxWidth())
            }
        },
        confirmButton = {
            Flat(confirm, color = if (which == "delete" || which == "report") Tc.danger else Tc.lime) {
                runCatching {
                    when (which) {
                        "verify" -> a.setVerified(c.index, !c.verified)
                        "rename" -> a.rename(c.index, input)
                        "report" -> reportText?.let { a.report(c.index, it) }
                        else -> { a.delete(c.index); m.screen = Screen.Chats }
                    }
                }.onFailure { m.note = it.message ?: "$it" }
                m.changed()
                close()
            }
        },
        dismissButton = { Flat("Cancel", color = Tc.dim, onClick = close) },
    )
}
