package app.termchat

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** The contact list, most recent first. */
@Composable
fun Chats(m: Model) {
    val a = m.account ?: return
    @Suppress("UNUSED_EXPRESSION") m.version
    val contacts = a.contacts().sortedByDescending { it.lastAt }
    val update = a.update()
    var menu by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("termchat", style = Tc.name)
                val link = if (a.connected()) "ONLINE" else "CONNECTING"
                Text("$link · ${a.nickname()} · ${a.fingerprint().take(5)}", style = Tc.caps.copy(color = if (a.connected()) Tc.lime else Tc.warn))
            }
            Text("+", style = Tc.name.copy(color = Tc.lime), modifier = Modifier.clickable { m.screen = Screen.Add }.padding(10.dp))
            Box {
                Text("⋮", style = Tc.name, modifier = Modifier.clickable { menu = true }.padding(10.dp))
                DropdownMenu(menu, { menu = false }) {
                    val item = @Composable { label: String, go: () -> Unit ->
                        DropdownMenuItem({ Text(label, style = Tc.mono) }, { menu = false; go() })
                    }
                    item("Your invite") { m.screen = Screen.Invite }
                    item("Add contact") { m.screen = Screen.Add }
                    item("Settings" + if (a.backedUp()) "" else "  ●") { m.screen = Screen.Settings }
                    item("What's new" + if (update != null) "  ●" else "") { m.screen = Screen.News }
                    item("Lock") { m.lock() }
                }
            }
        }
        if (update != null) Banner("termchat $update is out: tap for how to update") { m.screen = Screen.News }
        if (!a.backedUp()) Banner("No backup yet: a lost phone means a lost identity") { m.screen = Screen.Settings }
        if (m.note.isNotEmpty()) Text(m.note, style = Tc.small.copy(color = Tc.warn), modifier = Modifier.padding(horizontal = 16.dp))
        Line()
        if (contacts.isEmpty()) {
            Column(Modifier.fillMaxWidth().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Text("No contacts yet.", style = Tc.mono)
                Text("Show your invite to a friend, or add theirs with +.", style = Tc.small)
            }
        }
        LazyColumn {
            items(contacts, key = { it.fingerprint }) { c ->
                Row(
                    Modifier.fillMaxWidth().clickable { m.screen = Screen.Chat(c.index) }.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Avatar(c.name)
                    Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                        Text(c.name, style = Tc.mono.copy(color = if (c.blocked) Tc.dim else Tc.text))
                        val state = when {
                            c.request -> "CONTACT REQUEST"
                            c.left -> "LEFT"
                            c.blocked -> "BLOCKED"
                            !c.verified -> "NOT VERIFIED"
                            else -> "ENCRYPTED"
                        }
                        Text(state, style = Tc.caps.copy(color = if (c.verified && !c.left) Tc.lime else Tc.warn))
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
        }
    }
}

@Composable
fun Banner(text: String, onClick: () -> Unit) {
    Text(
        text,
        style = Tc.small.copy(color = Tc.lime),
        modifier = Modifier.fillMaxWidth().clickable(onClick = onClick).background(Tc.surface).padding(horizontal = 16.dp, vertical = 10.dp),
    )
}
