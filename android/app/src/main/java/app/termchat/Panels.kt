package app.termchat

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.os.PersistableBundle
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import kotlinx.coroutines.delay
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.termchat.engine.changelog
import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import org.unifiedpush.android.connector.UnifiedPush
import java.io.File

@Composable
private fun Panel(m: Model, title: String, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            IconTap("back", "Back to chats") { m.screen = Screen.Chats }
            Text(title, style = Tc.name)
        }
        Line()
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            content = content,
        )
    }
}

/** The clipboard, marked sensitive so Android does not preview it. */
fun copy(context: Context, text: String) {
    val clip = ClipData.newPlainText("termchat", text)
    clip.description.extras = PersistableBundle().apply { putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true) }
    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
}

internal fun qr(text: String): Bitmap {
    val bits = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 0, 0)
    val bmp = Bitmap.createBitmap(bits.width, bits.height, Bitmap.Config.ARGB_8888)
    for (x in 0 until bits.width) for (y in 0 until bits.height) {
        bmp.setPixel(x, y, if (bits[x, y]) 0xFF000000.toInt() else 0xFFFFFFFF.toInt())
    }
    return bmp
}

@Composable
fun Invite(m: Model) = Panel(m, "Your invite") {
    val a = m.account ?: return@Panel
    // One-time by default; the public one is reusable until renewed (ADR-013);
    // an incognito one comes from a new identity of its own (ADR-014).
    var kind by remember { mutableStateOf("once") }
    // `Err` until the invite queue exists, just after the first connect (ADR-023).
    val get = { f: () -> String -> runCatching(f).getOrElse { m.note = it.message ?: "$it"; "" } }
    var invite by remember { mutableStateOf(get { a.invite() }) }
    var nick by remember { mutableStateOf("") }
    // An incognito identity is being made: its invite comes as Event.Invite.
    var making by remember { mutableStateOf(false) }
    val context = LocalContext.current
    Text(
        when (kind) {
            "public" -> "Reusable: anyone with it can ask to add you. Renew it to stop the old one."
            "incognito" -> "Works once, from a new identity used only with them: its own keys and its own name."
            else -> "Works once: show it to one friend, or send them the text."
        },
        style = Tc.small,
    )
    LaunchedEffect(m.readyInvite) {
        m.readyInvite?.let { invite = it; kind = "incognito"; m.readyInvite = null; making = false }
    }
    if (invite.isEmpty()) {
        Text("Connecting to the relay; try again in a moment.", style = Tc.small)
        return@Panel
    }
    Image(
        qr(invite).asImageBitmap(), "invite QR code",
        filterQuality = FilterQuality.None,
        modifier = Modifier.fillMaxWidth().aspectRatio(1f).background(Tc.text).padding(12.dp),
    )
    SelectionContainer { Text(invite, style = Tc.small.copy(color = Tc.text)) }
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Flat("Copy") { copy(context, invite); m.note = "invite copied" }
        Flat("Share", color = Tc.dim) {
            context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, invite), null))
        }
    }
    Text("Your fingerprint  ${runCatching { a.inviteFingerprint(invite) }.getOrDefault("")}", style = Tc.small)
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        when (kind) {
            "public" -> {
                Flat("One-time", color = Tc.dim) { kind = "once"; invite = get { a.invite() } }
                Flat("Renew", color = Tc.dim) { invite = get { a.publicInvite(true) }; m.note = "new public invite: the old one no longer works" }
            }
            "incognito" -> Flat("One-time", color = Tc.dim) { kind = "once"; invite = get { a.invite() } }
            else -> {
                Flat("Public", color = Tc.dim) { kind = "public"; invite = get { a.publicInvite(false) } }
                Flat("Revoke unused", color = Tc.dim) { m.note = "${a.revokeInvites()} unused one-time invite(s) revoked"; invite = get { a.invite() } }
            }
        }
    }
    if (kind == "once") {
        Line()
        Text("INCOGNITO", style = Tc.caps)
        Text(
            "A new identity for one contact, so contacts cannot link you. " +
                if (a.tor()) "Over Tor the relay cannot tell it is you." else "Without Tor the relay can link it to you by IP address.",
            style = Tc.small,
        )
        Field(nick, { nick = it }, "Name they will see (empty: random)", Modifier.fillMaxWidth())
        // The invite comes as Event.Invite once its queue exists (ADR-023);
        // until then the button gives way to a line saying so.
        if (making) {
            Text("making a new identity; its invite opens in a moment", style = Tc.small.copy(color = Tc.warn))
        } else {
            Flat("Incognito invite") {
                runCatching { a.incognitoInvite(nick) }
                    .onSuccess { making = true }
                    .onFailure { m.note = it.message ?: "$it" }
            }
        }
    }
}

@Composable
fun Add(m: Model) = Panel(m, "Add contact") {
    val a = m.account ?: return@Panel
    var invite by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    val add = { text: String, incognito: Boolean ->
        runCatching { a.add(text, incognito) }
            .onSuccess { if (incognito) m.syncPush(); m.screen = Screen.Chat(it) }
            .onFailure { error = it.message ?: "$it" }
    }
    // The camera is used only here, only after this tap (ADR-011).
    val scan = rememberLauncherForActivityResult(ScanContract()) { r -> r.contents?.let { add(it, false) } }
    Text("Scan their code, or paste their invite.", style = Tc.small)
    Flat("Scan QR code") {
        scan.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false).setPrompt("").setOrientationLocked(false))
    }
    Field(invite, { invite = it }, "termchat:…", Modifier.fillMaxWidth(), singleLine = false)
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Flat("Add", enabled = invite.isNotBlank()) { add(invite, false) }
        Flat("Add incognito", color = Tc.dim, enabled = invite.isNotBlank()) { add(invite, true) }
    }
    Text("Incognito: they get a new identity of yours, used only with them.", style = Tc.small)
    if (error.isNotEmpty()) Text(error, style = Tc.small.copy(color = Tc.warn))
}

@Composable
fun Bridges(m: Model) = Panel(m, "Tor bridges") {
    val a = m.account ?: return@Panel
    // Starts empty: the saved lines hold secrets and are shown only in part.
    var text by remember { mutableStateOf("") }
    var result by remember { mutableStateOf("") }
    Text(
        "Where Tor is blocked, it can enter through bridges: unlisted relays behind a disguise. " +
            "Get them at bridges.torproject.org or from @GetBridgesBot in Telegram, and paste them here, one per line. " +
            "Saving replaces the list; saving nothing turns bridges off.",
        style = Tc.small,
    )
    for (b in a.bridges()) Text(b, style = Tc.mono)
    Field(text, { text = it }, "obfs4 … / webtunnel … / snowflake …", Modifier.fillMaxWidth(), singleLine = false)
    Flat("Save") {
        m.slow({ a.setBridges(text) }) { r ->
            result = r.fold({ it }, { it.message ?: "$it" })
            if (r.isSuccess) text = ""
        }
    }
    if (result.isNotEmpty()) Text(result, style = Tc.small)
}

@Composable
fun News(m: Model) = Panel(m, "What's new") {
    val a = m.account
    LaunchedEffect(Unit) { a?.newsSeen() }
    a?.update()?.let {
        Text("termchat.app $it is out", style = Tc.name.copy(color = Tc.lime))
        Text("Update from where you installed it: F-Droid, or the APK on github.com/prophecy1345/termchat/releases.", style = Tc.mono)
        Line()
    }
    val entries = remember {
        // Preserve headings, paragraphs and bullets, and let Compose wrap their text.
        changelog().trim().split(Regex("""\r?\n(?:[ \t]*\r?\n)+|\r?\n(?=## |- )"""))
            .map { entry -> entry.lineSequence().joinToString(" ") { it.trim() } }
    }
    for (entry in entries) {
        when {
            entry.startsWith("## ") -> Text(entry.removePrefix("## "), style = Tc.name, modifier = Modifier.padding(top = 8.dp))
            entry.isBlank() -> {}
            else -> Text(
                entry.replace("**", "").replace("`", ""),
                style = Tc.small.copy(color = Tc.text),
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
fun Settings(m: Model) = Panel(m, "Settings") {
    val a = m.account ?: return@Panel
    // Rows that read the engine (Connect the open chat at once) redraw on a change.
    @Suppress("UNUSED_EXPRESSION") m.version
    val context = LocalContext.current
    var form by remember { mutableStateOf<String?>(null) }
    var f1 by remember { mutableStateOf("") }
    var f2 by remember { mutableStateOf("") }
    var f3 by remember { mutableStateOf("") }
    var result by remember { mutableStateOf("") }
    fun open(which: String) { form = which; f1 = ""; f2 = ""; f3 = ""; result = "" }

    // Backup: written to a private temporary file, copied to where the user picked, then erased.
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        uri ?: return@rememberLauncherForActivityResult
        val pw = f2
        val tmp = File(context.cacheDir, "backup")
        m.slow({
            a.backup(tmp.absolutePath, pw)
            context.contentResolver.openOutputStream(uri, "wt")!!.use { o -> tmp.inputStream().use { it.copyTo(o) } }
            context.contentResolver.takePersistableUriPermission(
                uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
        }) { r ->
            tmp.delete()
            f2 = ""; f3 = ""
            r.onSuccess {
                m.rememberBackup(uri)
                result = "Backup written. Move it off this phone, to storage you control: a wipe erases it only while it stays where you saved it, and a cloud keeps copies of its own."
                form = null
            }.onFailure { result = it.message ?: "$it" }
        }
    }

    val allow = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> m.notifyOn(ok) }
    Text("THEME", style = Tc.caps)
    var showThemes by remember { mutableStateOf(false) }
    MenuRow("Colours: ${Tc.theme}", "The same themes as in the terminal") { showThemes = !showThemes }
    if (showThemes) {
        for (t in remember { app.termchat.engine.themes() }) {
            Row(
                Modifier.fillMaxWidth().clickable { m.useTheme(t.name) }.padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                // A swatch: background, bubble, accent.
                for (i in listOf(1, 3, 0)) {
                    Box(Modifier.size(18.dp).background(androidx.compose.ui.graphics.Color(0xFF000000 or t.colors[i].toLong())))
                }
                Box(Modifier.padding(start = 12.dp).size(12.dp)) { if (t.name == Tc.theme) PixelIcon("check", Tc.lime) }
                Text(
                    t.name,
                    style = Tc.mono.copy(color = if (t.name == Tc.theme) Tc.lime else Tc.text),
                    modifier = Modifier.padding(start = 8.dp),
                )
            }
        }
    }
    Line()
    Text("NOTIFICATIONS", style = Tc.caps)
    MenuRow(
        "New messages: ${if (m.notify) "on" else "off"}",
        "\"termchat.app: New message\", with no name and no text",
    ) {
        if (m.notify) m.notifyOn(false)
        else if (android.os.Build.VERSION.SDK_INT >= 33) allow.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        else m.notifyOn(true)
    }
    MenuRow(
        "Delivery receipts: ${if (m.receipts) "on" else "off"}",
        "Contacts see two checks when their message is on your phone. " +
            "The relay can link your conversation by the timing.",
    ) { m.receiptsOn(!m.receipts, m.readReceipts) }
    MenuRow(
        "Read receipts: ${if (m.readReceipts) "on" else "off"}",
        "Contacts see when you read their messages, and you see when they read yours. " +
            "Off: neither.",
    ) { m.receiptsOn(m.receipts, !m.readReceipts) }
    MenuRow(
        "Stay connected in the background: ${if (m.stay) "on" else "off"}",
        "Messages arrive while the app is closed. Costs battery, shows a permanent " +
            "\"termchat.app is connected\" notice, and keeps your keys in memory: " +
            "the app no longer locks itself in the background.",
    ) { m.stayOn(!m.stay) }
    MenuRow(
        "Lock after ${m.lockMinutes} min without a touch",
        "Tap to change: 1, 5, 15 or 30 minutes. A longer time gives whoever " +
            "picks up the unlocked phone more time to read.",
    ) { m.nextLock() }
    val activity = LocalContext.current as android.app.Activity
    val startPush = {
        UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { ok ->
            if (ok) { UnifiedPush.register(context); m.syncPush() }
            else m.note = "Install a UnifiedPush distributor first, for example ntfy."
        }
    }
    val allowThenPush = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        if (ok) startPush() else m.note = "Without notifications a wake-up cannot show anything."
    }
    MenuRow(
        "Wake-ups through UnifiedPush: ${if (m.push != null) "on" else "off"}",
        "Needs a distributor app such as ntfy, with battery optimisation off for it. " +
            "When a message waits, the relay pings " +
            "your push server, and the phone shows \"New message\". The relay keeps the " +
            "push address; the push server sees when you get messages, not from whom.",
    ) {
        if (m.push != null) {
            m.stopPush()
        } else if (android.os.Build.VERSION.SDK_INT >= 33) {
            // A wake-up is shown as a notification: ask for that first.
            allowThenPush.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        } else {
            startPush()
        }
    }
    Line()
    Text("SECURITY", style = Tc.caps)
    MenuRow("Back up your identity", "Identity and contacts, no messages. Keep it off this phone.", dot = !a.backedUp()) { open("backup") }
    MenuRow("Change password") { open("password") }
    MenuRow("Duress password", "Typing it at unlock erases everything") { open("duress") }
    MenuRow("Wipe everything from this phone", color = Tc.danger) { open("wipe") }
    MenuRow(
        "Tor: ${if (a.tor()) "on" else "off"}",
        if (a.tor()) "Only through Tor: the relay does not see your IP" else "Direct: the relay and your provider see your IP",
    ) { result = a.toggleTor(); m.changed() }
    MenuRow(
        "Tor bridges: ${a.bridges().size}",
        "Where Tor itself is blocked: obfs4, webtunnel or snowflake bridges.",
    ) { m.screen = Screen.Bridges }
    MenuRow(
        "Connections: ${if (a.sharedLink()) "one for all contacts" else "one per contact"}",
        if (a.sharedLink()) "Less battery, but the relay sees which queues are yours"
        else "The relay cannot link your contacts' queues by connection; uses more battery",
    ) { result = runCatching { a.setSharedLink(!a.sharedLink()) }.getOrElse { it.message ?: "$it" }; m.changed() }
    MenuRow(
        "Connect the open chat at once: ${if (a.hurryOpen()) "on" else "off"}",
        "With Tor, chats connect over 30 s after unlock so the relay cannot tie them together. " +
            "On: the chat you open skips that wait, and the relay may see which chat you opened.",
    ) { a.setHurryOpen(!a.hurryOpen()); m.changed() }
    Line()
    Text("ABOUT", style = Tc.caps)
    MenuRow("What's new") { m.screen = Screen.News }
    MenuRow("termchat.app ${app.termchat.engine.version()}", "Your fingerprint ${a.fingerprint()}") {}

    if (result.isNotEmpty() && form == null) Text(result, style = Tc.small.copy(color = Tc.lime))
    val which = form ?: return@Panel
    AlertDialog(
        onDismissRequest = { form = null },
        containerColor = Tc.surface,
        shape = corner,
        title = {
            Text(
                when (which) {
                    "backup" -> "Back up your identity"
                    "password" -> "Change password"
                    "duress" -> "Duress password"
                    else -> "Wipe everything"
                },
                style = Tc.name,
            )
        },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                when (which) {
                    "backup" -> {
                        Text("A password for the backup file. Write it down.", style = Tc.small)
                        Field(f2, { f2 = it }, "Backup password", Modifier.fillMaxWidth(), secret = true)
                        Field(f3, { f3 = it }, "Repeat", Modifier.fillMaxWidth(), secret = true)
                        Flat("Choose where to save", enabled = !m.busy) {
                            if (f2 != f3) result = "passwords do not match" else save.launch("termchat-backup")
                        }
                    }
                    "password", "duress" -> {
                        val duress = which == "duress"
                        if (duress) Text(
                            "Entering the duress password at unlock silently erases everything and shows the first-run screen. " +
                                "It does NOT help if someone copied the phone first. The code is public, so they may know the feature. " +
                                "Destroying data after seizure can be a crime where you live. Leave it empty to turn it off.",
                            style = Tc.small.copy(color = Tc.warn),
                        )
                        Field(f1, { f1 = it }, "Current password", Modifier.fillMaxWidth(), secret = true)
                        Field(f2, { f2 = it }, if (duress) "Duress password" else "New password", Modifier.fillMaxWidth(), secret = true)
                        Field(f3, { f3 = it }, "Repeat", Modifier.fillMaxWidth(), secret = true)
                        Flat(if (m.busy) "…" else "Save", enabled = !m.busy) {
                            if (f2 != f3) { result = "passwords do not match"; return@Flat }
                            val (cur, new) = f1 to f2
                            m.slow({
                                if (duress) a.setDuress(cur, new.ifEmpty { null })
                                else { a.changePassword(cur, new); "Password changed." }
                            }) { r ->
                        // The passwords leave the screen's memory at once.
                        f1 = ""; f2 = ""; f3 = ""
                        r.onSuccess { result = it; form = null }.onFailure { result = it.message ?: "$it" }
                    }
                        }
                    }
                    else -> {
                        Text(
                            "Erases keys, contacts and history from this phone. No undo.\n" +
                                "WIPE deletes the account: contacts are told you left and the relay deletes your queues.\n" +
                                "MOVED (you restored on another device): erases only this phone and tells nobody.",
                            style = Tc.small.copy(color = Tc.warn),
                        )
                        Field(f1, { f1 = it }, "Type WIPE or MOVED", Modifier.fillMaxWidth())
                        Flat("Wipe", color = Tc.danger) {
                            when (f1.trim()) {
                                "WIPE" -> { form = null; if (a.leave(false)) m.leaving = true }
                                "MOVED" -> { form = null; a.leave(true) }
                                else -> result = "type WIPE or MOVED in capitals"
                            }
                        }
                    }
                }
                if (result.isNotEmpty()) Text(result, style = Tc.small.copy(color = Tc.warn))
            }
        },
        confirmButton = {},
        dismissButton = { Flat("Cancel", color = Tc.dim) { form = null } },
    )
}

/** ADR-028: this new device's link code, to scan or paste on the main one. */
@Composable
fun LinkCode(m: Model) = Panel(m, "Link to my main device") {
    val a = m.account ?: return@Panel
    @Suppress("UNUSED_EXPRESSION") m.version
    // Linked: the code has done its job.
    if (!a.isPrimary()) {
        LaunchedEffect(Unit) { m.screen = Screen.Chats }
        return@Panel
    }
    // Right after setup the relay is not reached yet: ask again each second.
    var code by remember { mutableStateOf("") }
    LaunchedEffect(a) {
        while (code.isEmpty()) {
            runCatching { a.linkCode() }
                .onSuccess { code = it; m.note = "" }
                .onFailure { m.note = it.message ?: "$it"; delay(1000) }
        }
    }
    val context = LocalContext.current
    Text("On your main device: Devices, then scan or paste this code. Then compare the fingerprint both screens show.", style = Tc.small)
    if (code.isEmpty()) return@Panel
    Image(
        qr(code).asImageBitmap(), "link QR code",
        filterQuality = FilterQuality.None,
        modifier = Modifier.fillMaxWidth().aspectRatio(1f).background(Tc.text).padding(12.dp),
    )
    SelectionContainer { Text(code, style = Tc.small.copy(color = Tc.text)) }
    Flat("Copy") { copy(context, code); m.note = "link code copied" }
}

/** ADR-028: our devices on the main one: add, label, remove. */
@Composable
fun Devices(m: Model) = Panel(m, "Devices") {
    val a = m.account ?: return@Panel
    @Suppress("UNUSED_EXPRESSION") m.version
    var code by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    var renaming by remember { mutableStateOf<UInt?>(null) }
    var removing by remember { mutableStateOf<UInt?>(null) }
    var label by remember { mutableStateOf("") }
    val add = { text: String ->
        runCatching { a.addDevice(text.trim()) }
            .onSuccess { code = ""; error = ""; m.changed() }
            .onFailure { error = it.message ?: "$it" }
    }
    val scan = rememberLauncherForActivityResult(ScanContract()) { r -> r.contents?.let { add(it) } }
    val main = a.isPrimary()
    Text("Up to four, this one included. Contacts write to each of them; labels stay on your own devices.", style = Tc.small)
    if (!main) Text("Your main device adds, renames and removes them.", style = Tc.small)
    a.devices().forEachIndexed { n, d ->
        Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text((d.label.ifEmpty { "device" }) + if (d.me) " (this one)" else "", style = Tc.mono)
                Text(d.fingerprint.take(11) + if (d.main) " · MAIN" else "", style = Tc.small)
            }
            if (main) Text("Rename", style = Tc.small.copy(color = Tc.lime), modifier = Modifier.clickable { renaming = n.toUInt(); label = d.label }.padding(8.dp))
            if (main && !d.me) Text("Remove", style = Tc.small.copy(color = Tc.danger), modifier = Modifier.clickable { removing = n.toUInt() }.padding(8.dp))
        }
    }
    if (!main) return@Panel
    Line()
    Text("Add a device: on it, Link to my main device; scan or paste its code here.", style = Tc.small)
    Flat("Scan QR code") {
        scan.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false).setPrompt("").setOrientationLocked(false))
    }
    Field(code, { code = it }, "termchat-link:…", Modifier.fillMaxWidth(), singleLine = false)
    Flat("Add", enabled = code.isNotBlank()) { add(code) }
    if (error.isNotEmpty()) Text(error, style = Tc.small.copy(color = Tc.warn))
    renaming?.let { row ->
        AlertDialog(
            onDismissRequest = { renaming = null },
            containerColor = Tc.surface,
            shape = corner,
            title = { Text("Rename a device", style = Tc.name) },
            text = { Field(label, { label = it }, "phone, laptop…", Modifier.fillMaxWidth()) },
            confirmButton = {
                Flat("Save") {
                    runCatching { a.setDeviceLabel(row, label) }.onFailure { m.note = it.message ?: "$it" }
                    renaming = null; m.changed()
                }
            },
            dismissButton = { Flat("Cancel", color = Tc.dim) { renaming = null } },
        )
    }
    removing?.let { row ->
        AlertDialog(
            onDismissRequest = { removing = null },
            containerColor = Tc.surface,
            shape = corner,
            title = { Text("Remove a device", style = Tc.name) },
            text = { Text("It keeps what it has, and reads and sends nothing more.", style = Tc.mono) },
            confirmButton = {
                Flat("Remove", color = Tc.danger) {
                    runCatching { a.removeDevice(row) }.onFailure { m.note = it.message ?: "$it" }
                    removing = null; m.changed()
                }
            },
            dismissButton = { Flat("Cancel", color = Tc.dim) { removing = null } },
        )
    }
}
