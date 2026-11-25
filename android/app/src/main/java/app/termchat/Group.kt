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
import app.termchat.engine.Picked

/** One small private group (ADR-027): like a chat, with members and an admin. */
@Composable
fun Group(m: Model, index: UInt) {
    val a = m.account ?: return
    @Suppress("UNUSED_EXPRESSION") m.version
    val g = a.groups().firstOrNull { it.index == index } ?: return run { m.screen = Screen.Chats }
    val messages = runCatching { a.groupMessages(index) }.getOrDefault(emptyList())
    var text by remember(index) { mutableStateOf(runCatching { a.groupDraft(index) }.getOrDefault("")) }
    // "members", "invite", "leave", "stepdown", "make:", "unmake:", "hand:" or
    // "handleave:" and a fingerprint (ADR-042), or a member's fingerprint to remove.
    var dialog by remember(index) { mutableStateOf<String?>(null) }
    var compose by remember(index) { mutableStateOf<Triple<UInt, Boolean, String>?>(null) }
    var unsend by remember(index) { mutableStateOf<UInt?>(null) }
    var forwarding by remember(index) { mutableStateOf<UInt?>(null) }
    // Messages picked to delete on this device; `null`: not picking.
    var picked by remember(index) { mutableStateOf<Set<Picked>?>(null) }
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
    // ADR-044: the received image open in the viewer.
    var viewing by remember { mutableStateOf<UInt?>(null) }
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

    viewing?.let { at ->
        // By the document's id: rows move as messages expire.
        val name = messages.firstOrNull { !it.mine && it.fileId == at }?.fileName ?: "image"
        ImageViewer(
            name,
            load = { a.viewGroupImage(index, at) },
            onSave = { viewing = null; saving = at; save.launch(name) },
            onClose = { viewing = null },
        )
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
                        "edit" -> { compose = Triple(at, true, ""); text = msg.text; a.setGroupDraft(index, "") }
                        "delete" -> unsend = at
                        "copy" -> copySensitive(context, msg.text)
                        "forward" -> forwarding = at
                        "pick" -> msg.id?.let { id ->
                            val p = Picked(id, msg.mine)
                            val now = picked.orEmpty()
                            picked = if (p in now) now - p else now + p
                        }
                        else -> {
                            val r = what.toUByte()
                            runCatching { a.reactGroup(index, at, if (msg.myReaction == r) null else r) }.onFailure(fail)
                            m.changed()
                        }
                    }
                }, group = true, view = { viewing = msg.fileId }, picking = picked != null, marked = msg.id?.let { Picked(it, msg.mine) in picked.orEmpty() } ?: false) { keep ->
                    val id = msg.fileId
                    if (id != null) if (keep) {
                        saving = id
                        save.launch(msg.fileName ?: "file")
                    } else {
                        runCatching { a.forgetGroupFile(index, id) }
                        m.changed()
                    }
                }
            }
        }
        if (m.note.isNotEmpty()) Text(m.note, style = Tc.small.copy(color = Tc.warn), modifier = Modifier.padding(horizontal = 12.dp))
        compose?.let { (_, edit, label) ->
            Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                IconText(
                    if (edit) "edit" else "reply",
                    if (edit) "Editing your message" else "Answering: $label",
                    Tc.mono, Tc.lime, Modifier.weight(1f),
                )
                IconTap("close", "Cancel") { if (edit) text = ""; compose = null }
            }
        }
        picked?.let { p ->
            PickBar(p.size, { dialog = "delete-here" }) { picked = null }
        }
        if (picked == null) Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (g.expiresIn != null) IconText("timer", span(g.expiresIn), Tc.small, Tc.lime)
            Box(
                Modifier.size(48.dp).border(1.dp, Tc.line, corner)
                    .clickable(enabled = !g.left) { attach.launch(arrayOf("*/*")) },
                contentAlignment = Alignment.Center,
            ) { Paperclip(if (g.left) Tc.line else Tc.dim) }
            Field(text, { text = it; if (compose?.second != true) a.setGroupDraft(index, it) }, if (g.left) "You are no longer in this group" else "Encrypted message", Modifier.weight(1f).heightIn(min = 48.dp), singleLine = false, maxLines = 6)
            Flat(if (compose?.second == true) "Save" else "Send", enabled = text.isNotBlank() && !g.left, color = Tc.lime, modifier = Modifier.height(48.dp)) {
                runCatching {
                    val cm = compose
                    when {
                        cm == null -> a.sendGroup(index, text)
                        cm.second -> a.editGroupMessage(index, cm.first, text)
                        else -> a.replyGroup(index, cm.first, text)
                    }
                }.onSuccess { a.setGroupDraft(index, ""); text = ""; compose = null; m.note = ""; m.changed() }.onFailure(fail)
            }
        }
    }
    when (val d = dialog) {
        null -> {}
        "members" -> Members(m, g, { dialog = it }) { dialog = null }
        "invite" -> InviteToGroup(m, g) { dialog = null }
        "export" -> ExportChat(m, app.termchat.engine.Place.Group(index)) { dialog = null }
        "clear" -> Confirm(m, "Clear the history?", "Delete every message here on this device? The others, and your other devices, keep theirs. What is still on its way stays. This cannot be undone.", "Clear", { dialog = null }) {
            runCatching { a.clearGroupHistory(index) }.onFailure(fail)
        }
        "delete-here" -> Confirm(m, "Delete the selected?", DELETE_HERE, "Delete", { dialog = null }) {
            runCatching { a.deleteGroupHere(index, picked.orEmpty().toList()) }.onFailure(fail)
            picked = null
        }
        "leave" -> Confirm(m, "Leave the group?", leaveText(g), "Leave", { dialog = null }) {
            runCatching { a.leaveGroup(index) }.onFailure(fail)
        }
        "stepdown" -> Confirm(m, "Step down?", "You stay a member; the main admin moves the group.", "Step down", { dialog = null }) {
            runCatching { a.stepDown(index) }.onFailure(fail)
        }
        else -> {
            val (what, fp) = if (':' in d) d.substringBefore(':') to d.substringAfter(':') else "remove" to d
            val who = runCatching { a.groupMembers(index) }.getOrDefault(emptyList()).firstOrNull { it.fingerprint == fp }
            val name = who?.name ?: ""
            when (what) {
                "make" -> Confirm(m, "Make $name an admin?", "They can invite their own contacts. Only you remove people, set the timer and make admins. Choose someone you trust: an admin can also cut a member off at the relay.", "Make admin", { dialog = null }) {
                    runCatching { a.makeAdmin(index, fp) }.onFailure(fail)
                }
                "unmake" -> Confirm(m, "Remove $name as admin?", "They stay a member. The group moves to a new queue, so the key they held opens nothing.", "Remove admin", { dialog = null }) {
                    runCatching { a.removeAdmin(index, fp) }.onFailure(fail)
                }
                "hand", "handleave" -> Confirm(
                    m, "Hand the group to $name?",
                    "They become the main admin and move the group to a new queue. You can no longer remove people, set the timer or make admins." +
                        if (what == "handleave") " Then you leave the group." else "",
                    "Hand over", { dialog = null },
                ) {
                    runCatching { a.handOverGroup(index, fp, what == "handleave") }.onFailure(fail)
                }
                else -> RemoveMember(m, index, d, name, fail) { dialog = null }
            }
        }
    }
    forwarding?.let { at -> ForwardTo(m, app.termchat.engine.Place.Group(index), at) { forwarding = null } }
    unsend?.let { at ->
        Confirm(m, "Delete the message?", "It goes for everyone in the group. A copy or screenshot someone made stays.", "Delete", { unsend = null }) {
            runCatching { a.deleteGroupMessage(index, at) }.onFailure(fail)
        }
    }
}

@Composable
private fun RemoveMember(m: Model, index: UInt, fp: String, name: String, fail: (Throwable) -> Unit, close: () -> Unit) {
    val a = m.account ?: return
    Confirm(m, "Remove $name?", "The group moves to a new queue: they read nothing from now on.", "Remove", close) {
        runCatching { a.removeFromGroup(index, fp) }.onFailure(fail)
    }
}

/** ADR-042, section 4: what leaving says about who runs the group after us. */
fun leaveText(g: GroupView): String {
    val base = "The others are told. The chat stays here, read-only."
    if (!g.mainAdmin) return base
    return base + (g.nextAdmin?.let { " $it becomes the main admin." }
        ?: " Nobody can change the group after you: make someone an admin first, or hand it over.")
}

@Composable
private fun GroupHeader(m: Model, g: GroupView, ask: (String) -> Unit) {
    val a = m.account ?: return
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        IconTap("back", "Back to chats") { m.screen = Screen.Chats }
        Avatar("# ${g.name}")
        Column(Modifier.weight(1f).padding(start = 12.dp).clickable { ask("members") }) {
            Text(g.name, style = Tc.name)
            Text(groupState(g) + if (g.mainAdmin) " · MAIN ADMIN" else if (g.admin) " · ADMIN" else "", style = Tc.caps.copy(color = if (g.left) Tc.dim else Tc.lime))
            // ADR-038: why its messages are not here yet.
            if (g.connecting) Text("CONNECTING…", style = Tc.caps.copy(color = Tc.warn))
        }
        Box {
            IconTap("menu", "Group menu") { menu = true }
            DropdownMenu(menu, { menu = false }) {
                val item = @Composable { label: String, go: () -> Unit ->
                    DropdownMenuItem({ Text(label, style = Tc.mono) }, { menu = false; go() })
                }
                item("Members") { ask("members") }
                item("Export") { ask("export") }
                item("Clear history") { ask("clear") }
                if (g.admin) item("Invite") { ask("invite") }
                if (g.mainAdmin) {
                    item("Disappearing: ${span(g.expiresIn)}") {
                        runCatching { a.nextGroupTimer(g.index) }.onFailure { m.note = it.message ?: "$it" }
                        m.changed()
                    }
                }
                KeepOpen("Mute: ${g.mute}") { runCatching { a.nextGroupMute(g.index) }; m.changed() }
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
                    if (g.admin) "You and the other admins added them. Others see them on your word: compare safety numbers in person."
                    else "The admins added them. Compare safety numbers in person to be sure.",
                    style = Tc.small,
                )
                members.forEach { p ->
                    var actions by remember { mutableStateOf(false) }
                    Row(Modifier.fillMaxWidth().border(1.dp, Tc.line, corner).padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
                        Avatar(p.name, 32.dp)
                        Column(Modifier.weight(1f).padding(start = 10.dp)) {
                            Text(p.name + if (p.me) " (you)" else "", style = Tc.mono)
                            Text(p.fingerprint.take(11) + if (p.mainAdmin) " · MAIN ADMIN" else if (p.admin) " · ADMIN" else "", style = Tc.small)
                        }
                        // ADR-042: the main admin acts on others; a helper steps down.
                        val stepDown = p.me && p.admin && !p.mainAdmin && g.admin
                        if ((g.mainAdmin && !p.me) || stepDown) Box {
                            IconTap("menu", "Member actions") { actions = true }
                            DropdownMenu(actions, { actions = false }) {
                                val item = @Composable { label: String, go: String ->
                                    DropdownMenuItem({ Text(label, style = Tc.mono) }, { actions = false; ask(go) })
                                }
                                if (stepDown) {
                                    item("Step down", "stepdown")
                                } else {
                                    if (p.admin) item("Remove admin", "unmake:${p.fingerprint}")
                                    else item("Make admin", "make:${p.fingerprint}")
                                    item("Hand over", "hand:${p.fingerprint}")
                                    item("Hand over and leave", "handleave:${p.fingerprint}")
                                    item("Remove", p.fingerprint)
                                }
                            }
                        }
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
                            IconText("check", "Invited", Tc.small, Tc.lime, Modifier.padding(6.dp))
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
