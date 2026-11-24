package app.termchat

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.graphics.asImageBitmap
import app.termchat.engine.ContactView
import app.termchat.engine.MessageView
import app.termchat.engine.Place
import app.termchat.engine.reactions

/** ADR-043: the largest document, as in the Rust core. */
private const val MAX_FILE = 25 shl 20

/** A picked document's name and bytes, at most 25 MiB (ADR-043): opaque, never opened. */
internal fun readDocument(context: android.content.Context, uri: android.net.Uri): Pair<String, ByteArray> {
    // Review 2026-11d, finding 5: another app's share could name our own
    // private files (file://, or a provider of ours). Only another app's
    // content goes out.
    val authority = uri.authority.orEmpty()
    require(uri.scheme == "content" && authority.isNotEmpty() && !authority.startsWith(context.packageName)) {
        "this file cannot be shared"
    }
    val r = context.contentResolver
    val name = r.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
        if (it.moveToFirst()) it.getString(0) else null
    } ?: "file"
    val bytes = r.openInputStream(uri)!!.use { s ->
        val out = java.io.ByteArrayOutputStream()
        val buf = ByteArray(8192)
        while (true) {
            val n = s.read(buf)
            if (n < 0) break
            out.write(buf, 0, n)
            if (out.size() > MAX_FILE) error("documents are limited to 25 MiB")
        }
        out.toByteArray()
    }
    return name to bytes
}

/** ADR-025: the seven reactions as arcade sprites, from the Rust core. */
internal val REACTIONS = reactions()

/** What Send does (ADR-025): answer message `at`, or put new text in our message `at`. */
private data class Compose(val at: UInt, val edit: Boolean, val label: String)

/** One chat (ADR-011, section 5). */
@Composable
fun Chat(m: Model, index: UInt) {
    val a = m.account ?: return
    @Suppress("UNUSED_EXPRESSION") m.version
    val c = a.contacts().firstOrNull { it.index == index } ?: return run { m.screen = Screen.Chats }
    val messages = runCatching { a.messages(index) }.getOrDefault(emptyList())
    // The draft comes back when the chat opens again, also after a lock.
    var text by remember(index) { mutableStateOf(runCatching { a.draft(index) }.getOrDefault("")) }
    var dialog by remember { mutableStateOf<String?>(null) }
    // ponytail: by index, so a message that expires meanwhile shifts it; ids if that bites.
    var compose by remember(index) { mutableStateOf<Compose?>(null) }
    var unsend by remember(index) { mutableStateOf<UInt?>(null) }
    var forwarding by remember(index) { mutableStateOf<UInt?>(null) }
    // Search runs here, over what is already on the phone; `null` is off.
    var query by remember(index) { mutableStateOf<String?>(null) }
    DisposableEffect(index) {
        a.open(index)
        // After a lock `a` is destroyed; calling it would crash the app.
        onDispose { if (m.account === a) a.open(null) }
    }
    LaunchedEffect(m.version) { a.open(index) }
    val context = LocalContext.current

    // ADR-017, ADR-043: a document goes as opaque bytes, read here in full;
    // above 1 MiB it is offered and goes when they ask.
    val attach = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri ?: return@rememberLauncherForActivityResult
        runCatching {
            val (name, bytes) = readDocument(context, uri)
            a.sendFile(index, name, bytes)
        }.onSuccess { m.note = "" }.onFailure { m.note = it.message ?: "$it" }
        m.changed()
    }
    // The received document being saved: its bytes go where the user picked, then are forgotten.
    var saving by remember { mutableStateOf<UInt?>(null) }
    // ADR-044: the received image open in the viewer.
    var viewing by remember { mutableStateOf<UInt?>(null) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        val msg = saving ?: return@rememberLauncherForActivityResult
        saving = null
        uri ?: return@rememberLauncherForActivityResult
        runCatching {
            val bytes = a.fileBytes(index, msg) ?: error("this file is gone")
            context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write(bytes) }
            a.forgetFile(index, msg)
        }.onSuccess { m.note = "Saved. termchat never opens files; check it before you do." }
            .onFailure { m.note = it.message ?: "$it" }
        m.changed()
    }

    viewing?.let { at ->
        val name = messages.getOrNull(at.toInt())?.fileName ?: "image"
        ImageViewer(
            name,
            load = { a.viewImage(index, at) },
            onSave = { viewing = null; saving = at; save.launch(name) },
            onClose = { viewing = null },
        )
    }
    Column(Modifier.fillMaxSize()) {
        Header(m, c) { if (it == "search") query = "" else dialog = it }
        Line()
        query?.let { q ->
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Field(q, { query = it }, "Search this chat", Modifier.weight(1f))
                IconTap("close", "Close the search") { query = null }
            }
        }
        val all = messages.withIndex().toList()
        val shown = query?.takeIf { it.isNotBlank() }?.let { q ->
            all.filter { !it.value.system && it.value.text.contains(q.trim(), ignoreCase = true) }
        } ?: all
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
            reverseLayout = true,
            verticalArrangement = Arrangement.spacedBy(10.dp),
            contentPadding = PaddingValues(vertical = 12.dp),
        ) {
            items(shown.asReversed()) { (i, msg) ->
                val at = i.toUInt()
                val act: (String) -> Unit = { what ->
                    when (what) {
                        "answer" -> compose = Compose(at, false, msg.text.lineSequence().first())
                        "edit" -> { compose = Compose(at, true, ""); text = msg.text; a.setDraft(index, "") }
                        "delete" -> unsend = at
                        "copy" -> copySensitive(context, msg.text)
                        "forward" -> forwarding = at
                        else -> {
                            // A reaction; the same one again takes it back.
                            val r = what.toUByte()
                            runCatching { a.react(index, at, if (msg.myReaction == r) null else r) }
                                .onFailure { m.note = it.message ?: "$it" }
                            m.changed()
                        }
                    }
                }
                val download = {
                    runCatching { a.downloadFile(index, at) }.onFailure { m.note = it.message ?: "$it" }
                    m.changed()
                }
                Bubble(msg, c.rich, act, download = download, view = { viewing = at }) { keep ->
                    if (keep) {
                        saving = at
                        save.launch(msg.fileName ?: "file")
                    } else {
                        runCatching { a.forgetFile(index, at) }
                        m.changed()
                    }
                }
            }
        }
        // ADR-027: their group invites, answered here.
        if (!c.request) a.groupInvites().filter { it.from == index }.forEach { inv ->
            Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp)) {
                Text("Invites you to the group \"${inv.name}\"", style = Tc.mono.copy(color = Tc.warn))
                Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Flat("Join") {
                        runCatching { a.acceptGroup(inv.index) }
                            .onSuccess { m.screen = Screen.Group(it) }
                            .onFailure { m.note = it.message ?: "$it" }
                        m.changed()
                    }
                    Flat("Decline", color = Tc.dim) { a.declineGroup(inv.index); m.changed() }
                }
            }
        }
        // ADR-045: an introduction waiting for our answer.
        if (!c.request) runCatching { a.pendingIntro(index) }.getOrNull()?.let { intro ->
            val who = "${intro.nickname}·${intro.fingerprint}"
            Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp)) {
                Text(if (intro.ask) "Wants to introduce you to $who" else "Introduces $who", style = Tc.mono.copy(color = Tc.warn))
                val answer = { what: String ->
                    runCatching { a.answerIntro(index, intro.id, what) }.onFailure { m.note = it.message ?: "$it" }
                    m.changed()
                }
                Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (intro.ask) {
                        Flat("Allow") { answer("allow") }
                        Flat("Incognito", color = Tc.dim) { answer("incognito") }
                        Flat("Decline", color = Tc.dim) { answer("decline") }
                    } else {
                        Flat("Add") {
                            runCatching { a.answerOffer(index, intro.id, true) }
                                .onSuccess { j -> j?.let { m.screen = Screen.Chat(it) } }
                                .onFailure { m.note = it.message ?: "$it" }
                            m.changed()
                        }
                        Flat("Ignore", color = Tc.dim) {
                            runCatching { a.answerOffer(index, intro.id, false) }.onFailure { m.note = it.message ?: "$it" }
                            m.changed()
                        }
                    }
                }
            }
        }
        if (c.request) {
            Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Flat("Accept") { runCatching { a.accept(index) }; m.changed() }
                Flat("Reject", color = Tc.danger) { dialog = "delete" }
            }
        }
        if (m.note.isNotEmpty()) Text(m.note, style = Tc.small.copy(color = Tc.warn), modifier = Modifier.padding(horizontal = 12.dp))
        compose?.let { cm ->
            Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                IconText(
                    if (cm.edit) "edit" else "reply",
                    if (cm.edit) "Editing your message" else "Answering: ${cm.label}",
                    Tc.mono, Tc.lime, Modifier.weight(1f),
                )
                IconTap("close", "Cancel") {
                    if (cm.edit) text = ""
                    compose = null
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            // The fading envelope cycles the disappearing timer, like Ctrl+T.
            Column(
                Modifier.size(48.dp).border(1.dp, Tc.line, corner)
                    .semantics { contentDescription = "Disappearing messages: ${span(c.expiresIn)}" }
                    .clickable {
                        runCatching { a.nextTimer(index) }.onSuccess { t ->
                            m.note = if (t == null) "Disappearing messages off"
                            else "New messages disappear after ${span(t)}, on both sides"
                        }
                        m.changed()
                    },
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                FadingEnvelope(if (c.expiresIn != null) Tc.lime else Tc.dim)
                if (c.expiresIn != null) Text(span(c.expiresIn), style = Tc.small.copy(color = Tc.lime))
            }
            val closed = c.left || c.blocked || c.needsInvite
            Box(
                Modifier.size(48.dp).border(1.dp, Tc.line, corner)
                    .clickable(enabled = !closed && !c.request) { attach.launch(arrayOf("*/*")) },
                contentAlignment = Alignment.Center,
            ) { Paperclip(if (closed || c.request) Tc.line else Tc.dim) }
            Field(
                text, { text = it; if (compose?.edit != true) a.setDraft(index, it) },
                when {
                    c.left -> "This contact left"
                    c.blocked -> "Blocked"
                    // ADR-023: from before anonymous queues.
                    c.needsInvite -> "Ask them for a new invite, or send yours"
                    else -> "Encrypted message"
                },
                Modifier.weight(1f), singleLine = false,
            )
            Flat(if (compose?.edit == true) "Save" else "Send", enabled = text.isNotBlank() && !closed, color = Tc.lime) {
                runCatching {
                    val cm = compose
                    when {
                        cm == null -> a.send(index, text)
                        cm.edit -> a.editMessage(index, cm.at, text)
                        else -> a.reply(index, cm.at, text)
                    }
                }
                    .onSuccess { a.setDraft(index, ""); text = ""; compose = null; m.note = ""; m.changed() }
                    .onFailure { m.note = it.message ?: "$it" }
            }
        }
    }
    when (val d = dialog) {
        null -> {}
        "export" -> ExportChat(m, Place.Chat(index)) { dialog = null }
        "clear" -> Confirm(m, "Clear the history?", "Delete every message here on this device? The others, and your other devices, keep theirs. What is still on its way stays. This cannot be undone.", "Clear", { dialog = null }) {
            runCatching { a.clearChat(index) }.onFailure { m.note = it.message ?: "$it" }
        }
        "introduce" -> Introduce(m, index) { dialog = null }
        else -> ContactDialog(m, c, d) { dialog = null }
    }
    forwarding?.let { at -> ForwardTo(m, Place.Chat(index), at) { forwarding = null } }
    unsend?.let { at ->
        AlertDialog(
            onDismissRequest = { unsend = null },
            containerColor = Tc.surface,
            shape = corner,
            title = { Text("Delete the message?", style = Tc.name) },
            text = { Text("It goes for both of you. Their app removes it; a copy or screenshot they made stays.", style = Tc.mono) },
            confirmButton = {
                Flat("Delete", color = Tc.danger) {
                    runCatching { a.deleteMessage(index, at) }.onFailure { m.note = it.message ?: "$it" }
                    unsend = null
                    m.changed()
                }
            },
            dismissButton = { Flat("Cancel", color = Tc.dim) { unsend = null } },
        )
    }
}

/** To the clipboard, marked sensitive: keyboards and clipboard previews keep it out of sight. */
internal fun copySensitive(context: android.content.Context, text: String) {
    val clip = ClipData.newPlainText("message", text)
    clip.description.extras = android.os.PersistableBundle().apply {
        putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
    }
    context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip)
}

/** Reaction `r` as its arcade sprite with a soft glow (ADR-025). */
@Composable
internal fun Sprite(r: Int, size: Dp) {
    val sprite = REACTIONS.getOrNull(r) ?: return
    val color = Tc.slots.getOrNull(sprite.slot.toInt()) ?: Tc.lime
    Canvas(Modifier.size(size)) {
        val px = this.size.width / 13f
        val p = sprite.pixels
        for (pass in 0..1) {
            for (k in 0 until p.size / 2) {
                val x = (p[2 * k].toInt() + 1) * px
                val y = (p[2 * k + 1].toInt() + 1) * px
                when (pass) {
                    0 -> drawRect(color.copy(alpha = 0.2f), Offset(x - px * 0.6f, y - px * 0.6f), Size(px * 2.2f, px * 2.2f))
                    else -> drawRect(color, Offset(x, y), Size(px, px))
                }
            }
        }
    }
}

/** The switches both the chat menu and a long press in the list offer. */
@Composable
internal fun ContactToggles(m: Model, c: ContactView, item: @Composable (String, () -> Unit) -> Unit) {
    val a = m.account ?: return
    // The menu stays open: each tap picks the next span (owner, 2026-11-23).
    KeepOpen("Mute: ${c.mute}") { runCatching { a.nextMute(c.index) }; m.changed() }
    item(if (c.pinned) "Unpin" else "Pin") { runCatching { a.setPinned(c.index, !c.pinned) }; m.changed() }
    item(if (c.archived) "Unarchive" else "Archive") { runCatching { a.setArchived(c.index, !c.archived) }; m.changed() }
    if (!c.notes) item(if (c.blocked) "Unblock" else "Block") { runCatching { a.setBlocked(c.index, !c.blocked) }; m.changed() }
}

@Composable
private fun Header(m: Model, c: ContactView, ask: (String) -> Unit) {
    val a = m.account ?: return
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        IconTap("back", "Back to chats") { m.screen = Screen.Chats }
        Avatar(c.name)
        Column(Modifier.weight(1f).padding(start = 12.dp)) {
            Text(c.name, style = Tc.name)
            val (state, ok) = when {
                c.notes -> "ONLY ON YOUR DEVICES" to true
                c.left -> "LEFT" to false
                c.blocked -> "BLOCKED" to false
                c.needsInvite -> "NEEDS A NEW INVITE" to false
                c.verified -> "ENCRYPTED · VERIFIED" to true
                else -> "ENCRYPTED · NOT VERIFIED" to false
            }
            Text(state, style = Tc.caps.copy(color = if (ok) Tc.lime else Tc.warn))
            // ADR-038: why its messages are not here yet.
            if (c.connecting) Text("CONNECTING…", style = Tc.caps.copy(color = Tc.warn))
            c.incognito?.let { Text("INCOGNITO AS ${it.uppercase()}", style = Tc.caps.copy(color = Tc.dim)) }
        }
        Box {
            IconTap("menu", "Chat menu") { menu = true }
            DropdownMenu(menu, { menu = false }) {
                val item = @Composable { label: String, go: () -> Unit ->
                    DropdownMenuItem({ Text(label, style = Tc.mono) }, { menu = false; go() })
                }
                item("Search") { ask("search") }
                item("Export") { ask("export") }
                item("Clear history") { ask("clear") }
                if (!c.request && !c.notes) item("Introduce") { ask("introduce") }
                if (!c.notes) item("Safety number") { ask("verify") }
                item("Rename") { ask("rename") }
                item("Disappearing: ${span(c.expiresIn)}") { runCatching { a.nextTimer(c.index) }; m.changed() }
                item("No history: ${if (c.noHistory) "on" else "off"}") { runCatching { a.setNoHistory(c.index, !c.noHistory) }; m.changed() }
                ContactToggles(m, c, item)
                if (!c.notes) {
                    item("Report to the relay operator") { ask("report") }
                    item("Delete") { ask("delete") }
                }
            }
        }
    }
}

@Composable
internal fun Bubble(msg: MessageView, rich: Boolean, act: (String) -> Unit, group: Boolean = false, download: () -> Unit = {}, view: () -> Unit = {}, file: (save: Boolean) -> Unit) {
    if (msg.system) {
        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
            val box = Modifier.border(1.dp, Tc.line, corner).padding(horizontal = 12.dp, vertical = 6.dp)
            when (val rest = msg.text.removePrefix("⚠ ").takeIf { msg.text.startsWith("⚠ ") }) {
                null -> Text(msg.text, style = Tc.small, modifier = box)
                else -> Row(box, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PixelIcon("warning", Tc.warn)
                    Text(rest, style = Tc.small.copy(color = Tc.warn))
                }
            }
        }
        return
    }
    var menu by remember { mutableStateOf(false) }
    val gone = Tc.mono.copy(color = Tc.dim, fontStyle = FontStyle.Italic)
    Column(Modifier.fillMaxWidth(), horizontalAlignment = if (msg.mine) Alignment.End else Alignment.Start) {
        // ADR-027: in a group, who wrote it.
        msg.author?.let { Text(it, style = Tc.small.copy(color = Tc.slots.getOrNull((msg.authorSlot ?: 11u).toInt()) ?: Tc.lime), modifier = Modifier.padding(start = 2.dp, bottom = 2.dp)) }
        Box {
            Column(
                Modifier
                    .widthIn(max = 300.dp)
                    .background(if (msg.mine) Tc.mine else Tc.theirs, corner)
                    // ADR-025: a long press for answers, reactions, edits and deletes.
                    .pointerInput(msg.pointable) { detectTapGestures(onLongPress = { if (!msg.deleted && msg.fileName == null) menu = true }) }
                    .padding(horizontal = 14.dp, vertical = 10.dp),
            ) {
                msg.quote?.let { IconText("reply", it, gone, Tc.dim) }
                when {
                    msg.deleted -> Text("message deleted", style = gone)
                    msg.fileName != null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        PixelIcon("document", Tc.lime, 14.dp)
                        Text(msg.text, style = Tc.mono)
                    }
                    else -> Text(msg.text, style = Tc.mono)
                }
            }
            DropdownMenu(menu, { menu = false }, containerColor = Tc.surface) {
                val item = @Composable { label: String, what: String ->
                    DropdownMenuItem({ Text(label, style = Tc.mono) }, { menu = false; act(what) })
                }
                if (rich && msg.pointable) {
                    Row(Modifier.padding(horizontal = 8.dp)) {
                        REACTIONS.indices.forEach { r ->
                            val on = msg.myReaction?.toInt() == r
                            Box(
                                Modifier.size(38.dp)
                                    .border(1.dp, if (on) Tc.lime else Tc.surface, corner)
                                    .clickable { menu = false; act("$r") },
                                contentAlignment = Alignment.Center,
                            ) { Sprite(r, 28.dp) }
                        }
                    }
                    item("Answer", "answer")
                }
                if (msg.fileName == null) item("Copy", "copy")
                if (msg.forwardable) item("Forward", "forward")
                if (rich && msg.pointable && msg.mine) {
                    item("Edit", "edit")
                    item(if (group) "Delete for everyone" else "Delete for both", "delete")
                }
            }
        }
        if (msg.fileUnsaved) {
            Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                // ADR-044: decoded only on this tap, by the Rust engine.
                if (msg.filePicture) Flat("View", color = Tc.lime) { view() }
                Flat("Save", color = Tc.lime) { file(true) }
                Flat("Discard", color = Tc.dim) { file(false) }
            }
        }
        // ADR-043: nothing comes until they ask; declining frees the sender's copy.
        if (msg.fileOffered) {
            Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Flat("Download", color = Tc.lime) { download() }
                Flat("Decline", color = Tc.dim) { file(false) }
            }
        }
        val failed = msg.mine && msg.status !in setOf("sent", "sending", "queued")
        val meta = when {
            failed -> "failed: ${msg.status}"
            msg.mine && msg.status != "sent" -> msg.status
            else -> age(msg.at)
        } + if (msg.edited) " · edited" else ""
        Row(Modifier.padding(top = 3.dp, start = 2.dp, end = 2.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(meta, style = Tc.small.copy(color = if (failed) Tc.danger else Tc.dim))
            // ADR-041: one check sent, two delivered, two in the accent colour read.
            if (msg.mine && msg.status == "sent" && !msg.system) {
                PixelIcon(if (msg.delivered || msg.read) "checks" else "check", if (msg.read) Tc.lime else Tc.dim, 12.dp)
            }
            if (msg.counts.isNotEmpty()) {
                // In a group: each reaction once, with how many chose it.
                msg.counts.forEach {
                    Sprite(it.reaction.toInt(), 18.dp)
                    if (it.count > 1u) Text("${it.count}", style = Tc.small)
                }
            } else {
                // Theirs, then ours.
                listOfNotNull(msg.theirReaction, msg.myReaction).forEach { Sprite(it.toInt(), 18.dp) }
            }
        }
    }
}

/** The timer button's icon: an envelope whose right half has already
 *  crumbled away, for messages that disappear. */
@Composable
private fun FadingEnvelope(color: androidx.compose.ui.graphics.Color) {
    Box(Modifier.size(width = 24.dp, height = 17.dp).drawBehind {
        val w = size.width
        val h = size.height
        val line = Stroke(width = 1.5.dp.toPx())
        // The whole half: left side, top and bottom edges, the flap down to the middle.
        val half = Path().apply {
            moveTo(w * 0.56f, 0f)
            lineTo(0f, 0f)
            lineTo(0f, h)
            lineTo(w * 0.56f, h)
            moveTo(0f, 0f)
            lineTo(w * 0.5f, h * 0.55f)
            lineTo(w * 0.6f, h * 0.44f)
        }
        drawPath(half, color, style = line)
        // Then it breaks up: shorter dashes, then specks that fade out.
        val sw = 1.5.dp.toPx()
        for ((from, to) in listOf(0.64f to 0.72f, 0.79f to 0.84f)) {
            drawLine(color, Offset(w * from, 0f), Offset(w * to, 0f), sw)
            drawLine(color, Offset(w * from, h), Offset(w * to, h), sw)
        }
        drawLine(color, Offset(w * 0.66f, h * 0.36f), Offset(w * 0.71f, h * 0.3f), sw)
        val speck = 0.9.dp.toPx()
        listOf(
            Triple(0.9f, 0f, 0.7f), Triple(0.92f, 1f, 0.6f),
            Triple(0.8f, 0.2f, 0.5f), Triple(0.97f, 0.35f, 0.35f),
            Triple(0.88f, 0.6f, 0.4f), Triple(1f, 0.8f, 0.25f),
        ).forEach { (x, y, alpha) ->
            drawCircle(color.copy(alpha = alpha), speck, Offset(w * x, h * y))
        }
    })
}

@Composable
internal fun ContactDialog(m: Model, c: ContactView, which: String, close: () -> Unit) {
    val a = m.account ?: return
    var input by remember { mutableStateOf("") }
    // ADR-006: exactly the text shown is what goes to the operator, even if
    // more arrives while the dialog is open.
    val reportText = remember { if (which == "report") runCatching { a.reportText(c.index) }.getOrNull() else null }
    // ADR-037: their screen's code, scanned here; the camera only after this tap.
    val code = remember { if (which == "verify") runCatching { a.safetyCode(c.index) }.getOrNull() else null }
    val scan = rememberLauncherForActivityResult(com.journeyapps.barcodescanner.ScanContract()) { r ->
        val scanned = r.contents ?: return@rememberLauncherForActivityResult
        m.note = if (runCatching { a.verifyByCode(c.index, scanned) }.getOrDefault(false)) {
            "The codes match: ${c.name} is verified on this phone."
        } else {
            "This code does NOT match ${c.name}. Nothing changed; do not trust this chat until you find out why."
        }
        m.changed()
        close()
    }
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
            Column(
                Modifier.verticalScroll(androidx.compose.foundation.rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(body, style = Tc.mono)
                if (which == "rename") Field(input, { input = it }, c.name, Modifier.fillMaxWidth())
                if (code != null && code.isNotEmpty()) {
                    Text("Or scan the code on their screen, in front of you. One they send you proves nothing.", style = Tc.small)
                    androidx.compose.foundation.Image(
                        qr(code).asImageBitmap(), "safety number QR code",
                        filterQuality = androidx.compose.ui.graphics.FilterQuality.None,
                        modifier = Modifier.fillMaxWidth().aspectRatio(1f).background(Tc.text).padding(12.dp),
                    )
                    Flat("Scan their code", color = Tc.lime) {
                        scan.launch(
                            com.journeyapps.barcodescanner.ScanOptions().setDesiredBarcodeFormats(com.journeyapps.barcodescanner.ScanOptions.QR_CODE)
                                .setBeepEnabled(false).setPrompt("").setOrientationLocked(false)
                        )
                    }
                }
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

/** Where a message goes when forwarded: any chat or group, as a new message of ours. */
@Composable
internal fun ForwardTo(m: Model, from: Place, at: UInt, done: () -> Unit) {
    val a = m.account ?: return done()
    val send = { to: Place ->
        runCatching { a.forward(from, at, to) }
            .onSuccess { m.note = "forwarded" }
            .onFailure { m.note = it.message ?: "$it" }
        m.changed()
        done()
    }
    AlertDialog(
        onDismissRequest = done,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text("Forward to", style = Tc.name) },
        text = {
            LazyColumn(Modifier.heightIn(max = 360.dp)) {
                items(a.contacts().filter { !it.request }) { c ->
                    Text(c.name, style = Tc.mono, modifier = Modifier.fillMaxWidth().clickable { send(Place.Chat(c.index)) }.padding(vertical = 10.dp))
                }
                items(a.groups().filter { !it.left }) { g ->
                    Text("# ${g.name}", style = Tc.mono, modifier = Modifier.fillMaxWidth().clickable { send(Place.Group(g.index)) }.padding(vertical = 10.dp))
                }
            }
        },
        confirmButton = { Flat("Cancel", color = Tc.dim) { done() } },
    )
}

/** ADR-045: pick whom contact `index` should meet; that one is asked first. */
@Composable
internal fun Introduce(m: Model, index: UInt, done: () -> Unit) {
    val a = m.account ?: return done()
    val others = a.contacts().filter { it.index != index && !it.request && !it.blocked && !it.left && !it.needsInvite }
    AlertDialog(
        onDismissRequest = done,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text("Introduce to", style = Tc.name) },
        text = {
            Column {
                Text("They are asked first and see this contact's nickname and fingerprint.", style = Tc.small)
                LazyColumn(Modifier.heightIn(max = 360.dp)) {
                    items(others) { c ->
                        Text(c.name, style = Tc.mono, modifier = Modifier.fillMaxWidth().clickable {
                            runCatching { a.introduce(index, c.index) }
                                .onSuccess { m.note = "asked; they answer in their chat" }
                                .onFailure { m.note = it.message ?: "$it" }
                            m.changed()
                            done()
                        }.padding(vertical = 10.dp))
                    }
                }
            }
        },
        confirmButton = { Flat("Cancel", color = Tc.dim) { done() } },
    )
}

/** The chat as a plain text file, after a warning that it is not encrypted. */
@Composable
internal fun ExportChat(m: Model, place: Place, done: () -> Unit) {
    val a = m.account ?: return done()
    val context = LocalContext.current
    val export = remember(place) { runCatching { a.export(place) }.getOrNull() } ?: return done()
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri ->
        if (uri != null) {
            runCatching { context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write(export.text.toByteArray()) } }
                .onSuccess { m.note = "Exported. The file is not encrypted: delete it when done." }
                .onFailure { m.note = it.message ?: "$it" }
            m.changed()
        }
        done()
    }
    AlertDialog(
        onDismissRequest = done,
        containerColor = Tc.surface,
        shape = corner,
        title = { Text("Export this chat", style = Tc.name) },
        text = { Text("A plain text file, with times in UTC. It is not encrypted: anyone who can open it reads the chat, disappearing messages included. Delete it when you are done.", style = Tc.mono) },
        confirmButton = { Flat("Choose where", color = Tc.lime) { save.launch(export.name) } },
        dismissButton = { Flat("Cancel", color = Tc.dim) { done() } },
    )
}
