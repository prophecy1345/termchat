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
            Text("‹", style = Tc.name, modifier = Modifier.clickable { m.screen = Screen.Chats }.padding(end = 12.dp))
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

private fun qr(text: String): Bitmap {
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
    val invite = remember { a.invite() }
    val context = LocalContext.current
    Text("Show this code to a friend, or send them the text. It holds only public keys.", style = Tc.small)
    Image(
        qr(invite).asImageBitmap(), "invite QR code",
        filterQuality = FilterQuality.None,
        modifier = Modifier.fillMaxWidth().aspectRatio(1f).background(Tc.text).padding(12.dp),
    )
    SelectionContainer { Text(invite, style = Tc.small.copy(color = Tc.text)) }
    Text("Your fingerprint  ${a.fingerprint()}", style = Tc.small)
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Flat("Copy") { copy(context, invite); m.note = "invite copied" }
        Flat("Share", color = Tc.dim) {
            context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, invite), null))
        }
    }
}

@Composable
fun Add(m: Model) = Panel(m, "Add contact") {
    val a = m.account ?: return@Panel
    var invite by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    val add = { text: String ->
        runCatching { a.add(text) }
            .onSuccess { m.screen = Screen.Chat(it) }
            .onFailure { error = it.message ?: "$it" }
    }
    // The camera is used only here, only after this tap (ADR-011).
    val scan = rememberLauncherForActivityResult(ScanContract()) { r -> r.contents?.let { add(it) } }
    Text("Scan their code, or paste their invite.", style = Tc.small)
    Flat("Scan QR code") {
        scan.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false).setPrompt(""))
    }
    Field(invite, { invite = it }, "termchat:…", Modifier.fillMaxWidth(), singleLine = false)
    Flat("Add", enabled = invite.isNotBlank()) { add(invite) }
    if (error.isNotEmpty()) Text(error, style = Tc.small.copy(color = Tc.warn))
}

@Composable
fun News(m: Model) = Panel(m, "What's new") {
    val a = m.account
    LaunchedEffect(Unit) { a?.newsSeen() }
    a?.update()?.let {
        Text("termchat $it is out", style = Tc.name.copy(color = Tc.lime))
        Text("Update from where you installed it: F-Droid, or the APK on github.com/prophecy1345/termchat/releases.", style = Tc.mono)
        Line()
    }
    for (line in remember { changelog().lines() }) {
        when {
            line.startsWith("## ") -> Text(line.removePrefix("## "), style = Tc.name, modifier = Modifier.padding(top = 8.dp))
            line.isBlank() -> {}
            else -> Text(line.replace("**", "").replace("`", ""), style = Tc.small.copy(color = Tc.text))
        }
    }
}

@Composable
fun Settings(m: Model) = Panel(m, "Settings") {
    val a = m.account ?: return@Panel
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
            r.onSuccess {
                m.rememberBackup(uri)
                result = "Backup written. Move it off this phone: a wipe erases it only while it stays where you saved it."
                form = null
            }.onFailure { result = it.message ?: "$it" }
        }
    }

    val allow = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> m.notifyOn(ok) }
    Text("NOTIFICATIONS", style = Tc.caps)
    MenuRow(
        "New messages: ${if (m.notify) "on" else "off"}",
        "\"termchat: New message\", with no name and no text",
    ) {
        if (m.notify) m.notifyOn(false)
        else if (android.os.Build.VERSION.SDK_INT >= 33) allow.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        else m.notifyOn(true)
    }
    MenuRow(
        "Stay connected in the background: ${if (m.stay) "on" else "off"}",
        "Messages arrive while the app is closed. Costs battery, shows a permanent " +
            "\"termchat is connected\" notice, and keeps your keys in memory: " +
            "the app no longer locks itself in the background.",
    ) { m.stayOn(!m.stay) }
    val activity = LocalContext.current as android.app.Activity
    val startPush = {
        UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { ok ->
            if (ok) UnifiedPush.register(context)
            else m.note = "Install a UnifiedPush distributor first, for example ntfy."
        }
    }
    val allowThenPush = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        if (ok) startPush() else m.note = "Without notifications a wake-up cannot show anything."
    }
    MenuRow(
        "Wake-ups through UnifiedPush: ${if (m.push != null) "on" else "off"}",
        "Needs a distributor app such as ntfy. When a message waits, the relay pings " +
            "your push server, and the phone shows \"New message\". The relay keeps the " +
            "push address; the push server sees when you get messages, not from whom.",
    ) {
        if (m.push != null) {
            UnifiedPush.unregister(context)
            m.pushEndpoint(null)
        } else if (android.os.Build.VERSION.SDK_INT >= 33) {
            // A wake-up is shown as a notification: ask for that first.
            allowThenPush.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        } else {
            startPush()
        }
    }
    Line()
    Text("SECURITY", style = Tc.caps)
    MenuRow("Back up your identity" + if (a.backedUp()) "" else "  ●", "Identity and contacts, no messages. Keep it off this phone.") { open("backup") }
    MenuRow("Change password") { open("password") }
    MenuRow("Duress password", "Typing it at unlock erases everything") { open("duress") }
    MenuRow("Wipe everything from this phone", color = Tc.danger) { open("wipe") }
    MenuRow("Tor", "Not on Android yet") {}
    Line()
    Text("ABOUT", style = Tc.caps)
    MenuRow("What's new") { m.screen = Screen.News }
    MenuRow("termchat ${app.termchat.engine.version()}", "Your fingerprint ${a.fingerprint()}") {}

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
                            }) { r -> r.onSuccess { result = it; form = null }.onFailure { result = it.message ?: "$it" } }
                        }
                    }
                    else -> {
                        Text(
                            "Erases keys, contacts and history from this phone. No undo.\n" +
                                "WIPE deletes the account: contacts are told you left and the relay forgets your key.\n" +
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
