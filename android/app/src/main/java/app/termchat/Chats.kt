package app.termchat

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
    val (archived, contacts) = a.contacts()
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
                Row(
                    Modifier.fillMaxWidth().clickable { m.screen = Screen.Chat(c.index) }.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Avatar(c.name)
                    Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            if (c.pinned) PixelIcon("pin", Tc.lime)
                            Text(c.name, style = Tc.mono.copy(color = if (c.blocked || c.archived) Tc.dim else Tc.text))
                        }
                        val state = when {
                            c.request -> "CONTACT REQUEST"
                            c.left -> "LEFT"
                            c.blocked -> "BLOCKED"
                            c.needsInvite -> "NEEDS A NEW INVITE"
                            // ADR-027: answered in their chat.
                            c.index in invitedBy -> "GROUP INVITE"
                            !c.verified -> "NOT VERIFIED"
                            else -> "ENCRYPTED"
                        }
                        Text(state, style = Tc.caps.copy(color = if (c.verified && !c.left && c.index !in invitedBy) Tc.lime else Tc.warn))
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
                Row(
                    Modifier.fillMaxWidth().clickable { m.screen = Screen.Group(g.index) }.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Avatar("# ${g.name}")
                    Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                        Text(g.name, style = Tc.mono.copy(color = if (g.left) Tc.dim else Tc.text))
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
                Line()
            }
        }
    }
    if (newGroup) NewGroup(m) { newGroup = false }
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
