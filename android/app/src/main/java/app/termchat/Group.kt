package app.termchat

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.termchat.engine.GroupView

/** One small private group (ADR-027): like a chat, with members and an admin. */
@Composable
fun Group(m: Model, index: UInt) {
    val a = m.account ?: return
    @Suppress("UNUSED_EXPRESSION") m.version
    val g = a.groups().firstOrNull { it.index == index } ?: return run { m.screen = Screen.Chats }
    val messages = runCatching { a.groupMessages(index) }.getOrDefault(emptyList())
    var text by remember { mutableStateOf("") }
    // "members", "invite", "leave", or a member's fingerprint to remove.
    var dialog by remember(index) { mutableStateOf<String?>(null) }
    var compose by remember(index) { mutableStateOf<Triple<UInt, Boolean, String>?>(null) }
    var unsend by remember(index) { mutableStateOf<UInt?>(null) }
    DisposableEffect(index) {
        a.openGroup(index)
        onDispose { if (m.account === a) a.openGroup(null) }
    }
    LaunchedEffect(m.version) { a.openGroup(index) }
    val fail: (Throwable) -> Unit = { m.note = it.message ?: "$it" }
    val context = androidx.compose.ui.platform.LocalContext.current
    // Documents in the group (ADR-027 amendment), as in a chat.
    val attach = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri ?: return@rememberLauncherForActivityResult
        runCatching {
            val (name, bytes) = readDocument(context, uri)
            a.sendGroupFile(index, name, bytes)
        }.onSuccess { m.note = "" }.onFailure(fail)
        m.changed()
    }
    var saving by remember { mutableStateOf<UInt?>(null) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        val msg = saving ?: return@rememberLauncherForActivityResult
        saving = null
        uri ?: return@rememberLauncherForActivityResult
        runCatching {
            val bytes = a.groupFileBytes(index, msg) ?: error("this file is gone")
            context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write(bytes) }
            a.forgetGroupFile(index, msg)
        }.onSuccess { m.note = "Saved. termchat never opens files; check it before you do." }.onFailure(fail)
        m.changed()
    }

    Column(Modifier.fillMaxSize()) {
        GroupHeader(m, g) { dialog = it }
        Line()
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
            reverseLayout = true,
            verticalArrangement = Arrangement.spacedBy(10.dp),
            contentPadding = PaddingValues(vertical = 12.dp),
        ) {
            items(messages.withIndex().toList().asReversed()) { (i, msg) ->
                val at = i.toUInt()
                Bubble(msg, true, { what ->
                    when (what) {
                        "answer" -> compose = Triple(at, false, msg.text.lineSequence().first())
                        "edit" -> { compose = Triple(at, true, ""); text = msg.text }
                        "delete" -> unsend = at
                        "copy" -> copySensitive(context, msg.text)
                        else -> {
                            val r = what.toUByte()
                            runCatching { a.reactGroup(index, at, if (msg.myReaction == r) null else r) }.onFailure(fail)
                            m.changed()
                        }
                    }
                }, group = true) { keep ->
                    if (keep) {
                        saving = at
                        save.launch(msg.fileName ?: "file")
                    } else {
                        runCatching { a.forgetGroupFile(index, at) }
                        m.changed()
                    }
                }
            }
        }
        if (m.note.isNotEmpty()) Text(m.note, style = Tc.small.copy(color = Tc.warn), modifier = Modifier.padding(horizontal = 12.dp))
        compose?.let { (_, edit, label) ->
            Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (edit) "✎ Editing your message" else "↳ Answering: $label",
                    style = Tc.mono.copy(color = Tc.lime), maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text("×", style = Tc.name, modifier = Modifier.clickable { if (edit) text = ""; compose = null }.padding(horizontal = 12.dp))
            }
        }
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (g.expiresIn != null) Text("⏱ ${span(g.expiresIn)}", style = Tc.small.copy(color = Tc.lime))
            Box(
                Modifier.size(48.dp).border(1.dp, Tc.line, corner)
                    .clickable(enabled = !g.left) { attach.launch(arrayOf("*/*")) },
                contentAlignment = Alignment.Center,
            ) { Text("📎", style = Tc.name) }
            Field(text, { text = it }, if (g.left) "You are no longer in this group" else "Encrypted message", Modifier.weight(1f), singleLine = false)
            Flat(if (compose?.second == true) "Save" else "Send", enabled = text.isNotBlank() && !g.left, color = Tc.lime) {
                runCatching {
                    val cm = compose
                    when {
                        cm == null -> a.sendGroup(index, text)
                        cm.second -> a.editGroupMessage(index, cm.first, text)
                        else -> a.replyGroup(index, cm.first, text)
                    }
                }.onSuccess { text = ""; compose = null; m.note = ""; m.changed() }.onFailure(fail)
            }
        }
    }
    when (val d = dialog) {
        null -> {}
        "members" -> Members(m, g, { dialog = it }) { dialog = null }
        "invite" -> InviteToGroup(m, g) { dialog = null }
        "leave" -> Confirm(m, "Leave the group?", "The others are told. The chat stays here, read-only.", "Leave", { dialog = null }) {
            runCatching { a.leaveGroup(index) }.onFailure(fail)
        }
        else -> {
            val who = runCatching { a.groupMembers(index) }.getOrDefault(emptyList()).firstOrNull { it.fingerprint == d }
            Confirm(m, "Remove ${who?.name ?: ""}?", "The group moves to a new queue: they read nothing from now on.", "Remove", { dialog = null }) {
                runCatching { a.removeFromGroup(index, d) }.onFailure(fail)
            }
        }
    }
    unsend?.let { at ->
        Confirm(m, "Delete the message?", "It goes for everyone in the group. A copy or screenshot someone made stays.", "Delete", { unsend = null }) {
            runCatching { a.deleteGroupMessage(index, at) }.onFailure(fail)
        }
    }
}

@Composable
private fun GroupHeader(m: Model, g: GroupView, ask: (String) -> Unit) {
    val a = m.account ?: return
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("‹", style = Tc.name, modifier = Modifier.clickable { m.screen = Screen.Chats }.padding(end = 12.dp))
        Avatar("# ${g.name}")
        Column(Modifier.weight(1f).padding(start = 12.dp).clickable { ask("members") }) {
            Text(g.name, style = Tc.name)
            Text(groupState(g) + if (g.admin) " · ADMIN" else "", style = Tc.caps.copy(color = if (g.left) Tc.dim else Tc.lime))
        }
        Box {
            Text("⋮", style = Tc.name, modifier = Modifier.clickable { menu = true }.padding(8.dp))
            DropdownMenu(menu, { menu = false }) {
                val item = @Composable { label: String, go: () -> Unit ->
                    DropdownMenuItem({ Text(label, style = Tc.mono) }, { menu = false; go() })
                }
                item("Members") { ask("members") }
                if (g.admin) {
                    item("Invite") { ask("invite") }
                    item("Disappearing: ${span(g.expiresIn)}") {
                        runCatching { a.nextGroupTimer(g.index) }.onFailure { m.note = it.message ?: "$it" }
                        m.changed()
                    }
                }
                if (!g.left) item("Leave") { ask("leave") }
            }
        }
    }
}

fun groupState(g: GroupView): String = when {
    g.left -> "NO LONGER A MEMBER"
    g.members == 0u -> "SETTING UP…"
    g.members == 1u -> "1 MEMBER"
    else -> "${g.members} MEMBERS"
}

@Composable
private fun Members(m: Model, g: GroupView, ask: (String) -> Unit, close: () -> Unit) {
    val a = m.account ?: return
    val members = runCatching { a.groupMembers(g.index) }.getOrDefault(emptyList())
    AlertDialog(
        onDismissRequest = close,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text("${g.name} · members", style = Tc.name) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    if (g.admin) "You added them. Others see them on your word: compare safety numbers in person."
                    else "The admin added them. Compare safety numbers in person to be sure.",
                    style = Tc.small,
                )
                members.forEach { p ->
                    Row(Modifier.fillMaxWidth().border(1.dp, Tc.line, corner).padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
                        Avatar(p.name, 32.dp)
                        Column(Modifier.weight(1f).padding(start = 10.dp)) {
                            Text(p.name + if (p.me) " (you)" else "", style = Tc.mono)
                            Text(p.fingerprint.take(11) + if (p.admin) " · ADMIN" else "", style = Tc.small)
                        }
                        if (g.admin && !p.me) Text("Remove", style = Tc.small.copy(color = Tc.danger), modifier = Modifier.clickable { ask(p.fingerprint) }.padding(6.dp))
                    }
                }
            }
        },
        confirmButton = { if (g.admin) Flat("Invite") { ask("invite") } },
        dismissButton = { Flat("Close", color = Tc.dim, onClick = close) },
    )
}

@Composable
private fun InviteToGroup(m: Model, g: GroupView, close: () -> Unit) {
    val a = m.account ?: return
    @Suppress("UNUSED_EXPRESSION") m.version
    var error by remember { mutableStateOf("") }
    val rows = runCatching { a.invitable(g.index) }.getOrDefault(emptyList())
    AlertDialog(
        onDismissRequest = close,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text("Invite to the group", style = Tc.name) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Your own contacts only. They join from their chat with you.", style = Tc.small)
                if (rows.isEmpty()) Text("No contact to invite.", style = Tc.mono.copy(color = Tc.dim))
                rows.forEach { c ->
                    Row(Modifier.fillMaxWidth().border(1.dp, Tc.line, corner).padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
                        Avatar(c.name, 32.dp)
                        Text(c.name, style = Tc.mono, modifier = Modifier.weight(1f).padding(start = 10.dp))
                        if (c.invited) {
                            Text("✓ Invited", style = Tc.small.copy(color = Tc.lime), modifier = Modifier.padding(6.dp))
                        } else {
                            Flat("Invite") {
                                error = runCatching { a.inviteToGroup(g.index, c.index) }.exceptionOrNull()?.message ?: ""
                                m.changed()
                            }
                        }
                    }
                }
                if (error.isNotEmpty()) Text(error, style = Tc.small.copy(color = Tc.danger))
            }
        },
        confirmButton = { Flat("Close", color = Tc.dim, onClick = close) },
    )
}

@Composable
fun Confirm(m: Model, title: String, body: String, yes: String, close: () -> Unit, go: () -> Unit) {
    AlertDialog(
        onDismissRequest = close,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text(title, style = Tc.name) },
        text = { Text(body, style = Tc.mono) },
        confirmButton = { Flat(yes, color = Tc.danger) { go(); close(); m.changed() } },
        dismissButton = { Flat("Cancel", color = Tc.dim, onClick = close) },
    )
}
