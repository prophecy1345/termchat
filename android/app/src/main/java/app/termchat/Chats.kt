package app.termchat

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.termchat.engine.ContactView

/** The contact list, most recent first. */
@Composable
fun Chats(m: Model) {
    val a = m.account ?: return
    @Suppress("UNUSED_EXPRESSION") m.version
    // Archived first under their own heading, then pinned on top of the rest (owner, 2026-11-22).
    // ADR-046 amendment 1: Notes has a row of its own, above the people.
    val all = a.contacts()
    val notes = all.firstOrNull { it.notes }
    val (archived, contacts) = all.filter { !it.notes }
        .sortedWith(compareBy<ContactView> { !it.pinned }.thenByDescending { it.lastAt })
        .partition { it.archived }
    val groups = a.groups().sortedByDescending { it.lastAt }
    val invitedBy = a.groupInvites().map { it.from }.toSet()
    var newGroup by remember { mutableStateOf(false) }
    val update = a.update()
    var menu by remember { mutableStateOf(false) }
    var foldArchived by rememberSaveable { mutableStateOf(false) }
    var foldContacts by rememberSaveable { mutableStateOf(false) }
    var foldGroups by rememberSaveable { mutableStateOf(false) }
    // A long press opens the row's menu without opening the chat (owner, 2026-11-22).
    var held by remember { mutableStateOf<String?>(null) }
    var dialog by remember { mutableStateOf<Pair<ContactView, String>?>(null) }
    var leaving by remember { mutableStateOf<UInt?>(null) }
    val item = @Composable { label: String, go: () -> Unit ->
        DropdownMenuItem({ Text(label, style = Tc.mono) }, { held = null; go() })
    }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("termchat.app", style = Tc.name)
                val link = if (a.connected()) "ONLINE" else "CONNECTING"
                Text("$link · ${a.nickname()} · ${a.fingerprint().take(5)}", style = Tc.caps.copy(color = if (a.connected()) Tc.lime else Tc.warn))
            }
            IconTap("plus", "Add contact", Tc.lime) { m.screen = Screen.Add }
            Box {
                IconTap("menu", "Menu") { menu = true }
                DropdownMenu(menu, { menu = false }) {
                    val item = @Composable { label: String, go: () -> Unit ->
                        DropdownMenuItem({ Text(label, style = Tc.mono) }, { menu = false; go() })
                    }
                    item("Your invite") { m.screen = Screen.Invite }
                    item("Add contact") { m.screen = Screen.Add }
                    if (a.isPrimary()) item("New group") { newGroup = true }
                    if (a.canLink()) item("Link to my main device") { m.screen = Screen.LinkCode }
                    if (a.isPrimary() || a.devices().size > 1) item("Devices") { m.screen = Screen.Devices }
                    MenuItem("Settings", dot = !a.backedUp()) { menu = false; m.screen = Screen.Settings }
                    MenuItem("What's new", dot = update != null) { menu = false; m.screen = Screen.News }
                    item("Lock") { m.lock() }
                }
            }
        }
        if (a.removed()) Banner("This device was removed from your account: it reads and sends nothing more.") {}
        if (update != null) Banner("termchat.app $update is out: tap for how to update") { m.screen = Screen.News }
        if (!a.backedUp()) Banner("No backup yet: a lost phone means a lost identity") { m.screen = Screen.Settings }
        if (m.note.isNotEmpty()) Text(m.note, style = Tc.small.copy(color = Tc.warn), modifier = Modifier.padding(horizontal = 16.dp))
        Line()
        if (contacts.isEmpty() && archived.isEmpty()) {
            Column(Modifier.fillMaxWidth().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Text("No contacts yet.", style = Tc.mono)
                Text("Show your invite to a friend, or add theirs with +.", style = Tc.small)
            }
        }
        LazyColumn {
            val row: @Composable (ContactView) -> Unit = { c ->
                Box {
                    // Under the row, at the right, clear of the screen edge (owner, 2026-11-23).
                    Box(Modifier.align(Alignment.BottomEnd).padding(end = 16.dp)) {
                        DropdownMenu(held == c.fingerprint, { held = null }) {
                            item("Rename") { dialog = c to "rename" }
                            ContactToggles(m, c, item)
                            item("Delete") { dialog = c to "delete" }
                        }
                    }
                    Row(
                        Modifier.fillMaxWidth()
                            .combinedClickable(onLongClick = { held = c.fingerprint }) { m.screen = Screen.Chat(c.index) }
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Avatar(c.name)
                        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                if (c.pinned) PixelIcon("pin", Tc.lime)
                                Text(c.name, style = Tc.mono.copy(color = if (c.blocked || c.archived) Tc.dim else Tc.text))
                                if (c.mute != "off") PixelIcon("muted", Tc.dim)
                            }
                            val state = when {
                                c.notes -> "ONLY ON YOUR DEVICES"
                                c.request -> "CONTACT REQUEST"
                                c.left -> "LEFT"
                                c.blocked -> "BLOCKED"
                                c.needsInvite -> "NEEDS A NEW INVITE"
                                // ADR-027: answered in their chat.
                                c.index in invitedBy -> "GROUP INVITE"
                                !c.verified -> "NOT VERIFIED"
                                else -> "ENCRYPTED"
                            }
                            Text(state, style = Tc.caps.copy(color = if (c.notes || (c.verified && !c.left && c.index !in invitedBy)) Tc.lime else Tc.warn))
                        }
                        Column(horizontalAlignment = Alignment.End) {
                            Text(age(c.lastAt), style = Tc.small)
                            if (c.unread > 0u) {
                                Box(Modifier.padding(top = 4.dp).background(Tc.lime, CircleShape).padding(horizontal = 7.dp, vertical = 1.dp)) {
                                    Text("${c.unread}", style = Tc.small.copy(color = Tc.bg))
                                }
                            }
                        }
                    }
                }
                Line()
            }
            // Each heading folds its section.
            val heading: (String, Int, Boolean, () -> Unit) -> Unit = { title, n, folded, flip ->
                item(key = "h$title") {
                    Row(
                        Modifier.fillMaxWidth().clickable(onClick = flip).padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        PixelIcon(if (folded) "folded" else "unfolded", Tc.dim, 10.dp)
                        Text("$title  $n", style = Tc.caps.copy(color = Tc.dim))
                    }
                    Line()
                }
            }
            notes?.let { n -> item(key = "notes") { row(n); Line() } }
            if (archived.isNotEmpty()) {
                heading("ARCHIVED", archived.size, foldArchived) { foldArchived = !foldArchived }
                if (!foldArchived) items(archived, key = { it.fingerprint }) { row(it) }
            }
            if (contacts.isNotEmpty()) {
                heading("CONTACTS", contacts.size, foldContacts) { foldContacts = !foldContacts }
                if (!foldContacts) items(contacts, key = { it.fingerprint }) { row(it) }
            }
            // ADR-027: small private groups, after the contacts.
            if (groups.isNotEmpty()) heading("GROUPS", groups.size, foldGroups) { foldGroups = !foldGroups }
            if (!foldGroups) items(groups, key = { "g${it.index}" }) { g ->
                Box {
                    // Under the row, at the right, clear of the screen edge (owner, 2026-11-23).
                    Box(Modifier.align(Alignment.BottomEnd).padding(end = 16.dp)) {
                        DropdownMenu(held == "g${g.index}", { held = null }) {
                            KeepOpen("Mute: ${g.mute}") { runCatching { a.nextGroupMute(g.index) }; m.changed() }
                            if (!g.left) item("Leave") { leaving = g.index }
                        }
                    }
                    Row(
                        Modifier.fillMaxWidth()
                            .combinedClickable(onLongClick = { held = "g${g.index}" }) { m.screen = Screen.Group(g.index) }
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Avatar("# ${g.name}")
                        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Text(g.name, style = Tc.mono.copy(color = if (g.left) Tc.dim else Tc.text))
                                if (g.mute != "off") PixelIcon("muted", Tc.dim)
                            }
                            Text(groupState(g), style = Tc.caps.copy(color = if (g.left) Tc.dim else Tc.lime))
                        }
                        Column(horizontalAlignment = Alignment.End) {
                            if (g.lastAt > 0u) Text(age(g.lastAt), style = Tc.small)
                            if (g.unread > 0u) {
                                Box(Modifier.padding(top = 4.dp).background(Tc.lime, CircleShape).padding(horizontal = 7.dp, vertical = 1.dp)) {
                                    Text("${g.unread}", style = Tc.small.copy(color = Tc.bg))
                                }
                            }
                        }
                    }
                }
                Line()
            }
        }
    }
    if (newGroup) NewGroup(m) { newGroup = false }
    dialog?.let { (c, which) -> ContactDialog(m, c, which) { dialog = null } }
    leaving?.let { g ->
        val view = a.groups().firstOrNull { it.index == g }
        Confirm(m, "Leave the group?", view?.let { leaveText(it) } ?: "The others are told. The chat stays here, read-only.", "Leave", { leaving = null }) {
            runCatching { a.leaveGroup(g) }.onFailure { m.note = it.message ?: "$it" }
        }
    }
}

@Composable
private fun NewGroup(m: Model, close: () -> Unit) {
    val a = m.account ?: return
    var name by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = close,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text("New group", style = Tc.name) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("A small private group, by invitation only, up to 32 people. You are its admin.", style = Tc.mono)
                Field(name, { name = it }, "Weekend trip", Modifier.fillMaxWidth())
                Text("Seen only by its members. Nobody can find the group.", style = Tc.small)
                if (error.isNotEmpty()) Text(error, style = Tc.small.copy(color = Tc.danger))
            }
        },
        confirmButton = {
            Flat("Create", enabled = name.isNotBlank()) {
                runCatching { a.createGroup(name.trim()) }
                    .onSuccess { close(); m.changed(); m.screen = Screen.Group(it) }
                    .onFailure { error = it.message ?: "$it" }
            }
        },
        dismissButton = { Flat("Cancel", color = Tc.dim, onClick = close) },
    )
}

@Composable
fun Banner(text: String, onClick: () -> Unit) {
    Text(
        text,
        style = Tc.small.copy(color = Tc.lime),
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick).background(Tc.surface).padding(horizontal = 16.dp, vertical = 10.dp),
    )
}

/** Where a share from another app goes: text becomes the chat's draft, a file is sent. */
@Composable
internal fun ShareTo(m: Model) {
    val a = m.account ?: return
    val share = m.share ?: return
    val chats = a.contacts().filter { !it.blocked && !it.left && !it.request && !it.needsInvite }
    val groups = a.groups().filter { !it.left }
    val what = share.name?.let { "$it (${(share.bytes?.size ?: 0) / 1024 + 1} KB)" }
        ?: "Text, ${share.text?.length ?: 0} characters"
    fun done(note: String, screen: Screen) {
        m.share = null
        m.note = note
        m.screen = screen
        m.changed()
    }
    AlertDialog(
        onDismissRequest = { m.share = null },
        containerColor = Tc.surface,
        shape = corner,
        title = { Text("Share to…", style = Tc.name) },
        text = {
            Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(what, style = Tc.small)
                if (share.name != null) Text("It goes as soon as you pick a chat. termchat never opens files.", style = Tc.small)
                else Text("It goes into the message field; you send it.", style = Tc.small)
                if (chats.isEmpty() && groups.isEmpty()) Text("No chat to share to yet.", style = Tc.mono.copy(color = Tc.dim))
                chats.forEach { c ->
                    Row(Modifier.fillMaxWidth().clickable {
                        runCatching {
                            if (share.bytes != null) a.sendFile(c.index, share.name ?: "file", share.bytes) else a.setDraft(c.index, share.text ?: "")
                        }.onSuccess { done(if (share.bytes != null) "Sent ${share.name}" else "", Screen.Chat(c.index)) }
                            .onFailure { done(it.message ?: "$it", Screen.Chats) }
                    }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Avatar(c.name, 32.dp)
                        Text(c.name, style = Tc.mono, modifier = Modifier.padding(start = 10.dp))
                    }
                }
                groups.forEach { g ->
                    Row(Modifier.fillMaxWidth().clickable {
                        runCatching {
                            if (share.bytes != null) a.sendGroupFile(g.index, share.name ?: "file", share.bytes) else a.setGroupDraft(g.index, share.text ?: "")
                        }.onSuccess { done(if (share.bytes != null) "Sent ${share.name}" else "", Screen.Group(g.index)) }
                            .onFailure { done(it.message ?: "$it", Screen.Chats) }
                    }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Avatar("# ${g.name}", 32.dp)
                        Text(g.name, style = Tc.mono, modifier = Modifier.padding(start = 10.dp))
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { Flat("Cancel", color = Tc.dim) { m.share = null } },
    )
}
