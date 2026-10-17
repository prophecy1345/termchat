package app.termchat

import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import app.termchat.engine.generatePassword
import app.termchat.engine.restore
import app.termchat.engine.setup
import app.termchat.engine.stateExists
import app.termchat.engine.unlock
import java.io.File

/** First run (create or restore) and the lock screen. */
@Composable
fun Start(m: Model) {
    var exists by remember(m.wipes) { mutableStateOf(stateExists(m.dir)) }
    var error by remember(m.wipes) { mutableStateOf(m.wipedNote) }
    var relay by remember { mutableStateOf("wss://relay.termchat.app/") }
    var nick by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    val context = LocalContext.current

    // Restore: copy the picked backup in, then it opens with its own password.
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri ?: return@rememberLauncherForActivityResult
        val tmp = File(context.cacheDir, "restore")
        runCatching {
            context.contentResolver.takePersistableUriPermission(
                uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
            context.contentResolver.openInputStream(uri)!!.use { i -> tmp.outputStream().use { i.copyTo(it) } }
            restore(m.dir, tmp.absolutePath)
            m.rememberBackup(uri)
        }.onSuccess {
            exists = true
            error = "restored: enter the backup's password"
        }.onFailure { error = it.message ?: "$it" }
        tmp.delete()
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Spacer(Modifier.height(24.dp))
        Text("termchat", style = Tc.name)
        Text(if (exists) "LOCKED" else "NEW IDENTITY", style = Tc.caps)
        if (!exists) {
            Text("No names, phones or emails: just a nickname. Keep the relay below or type your own.", style = Tc.small)
            Field(relay, { relay = it }, "Relay URL", Modifier.fillMaxWidth())
            Field(nick, { nick = it }, "Nickname", Modifier.fillMaxWidth())
        }
        Field(password, { password = it }, "Password", Modifier.fillMaxWidth(), secret = true)
        if (!exists) {
            Field(repeat, { repeat = it }, "Repeat password", Modifier.fillMaxWidth(), secret = true)
            Text(
                "Generate a passphrase",
                style = Tc.small.copy(color = Tc.lime),
                modifier = Modifier.padding(vertical = 4.dp).clickable {
                    generatePassword().let { password = it; repeat = it }
                    error = "write it down: $password"
                },
            )
        }
        if (error.isNotEmpty()) Text(error, style = Tc.small.copy(color = Tc.warn))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Flat(if (m.busy) "…" else if (exists) "Unlock" else "Create", enabled = !m.busy) {
                if (!exists && password != repeat) {
                    error = "passwords do not match"
                    return@Flat
                }
                val pw = password
                m.slow({
                    if (exists) unlock(m.dir, pw, m) else setup(m.dir, relay, nick, pw, m)
                }) { result ->
                    password = ""
                    repeat = ""
                    result.onSuccess { a ->
                        m.wipedNote = ""
                        error = ""
                        // No account back: the duress password erased everything (ADR-003).
                        if (a == null) m.wiped("") else m.opened(a)
                    }.onFailure { error = it.message ?: "$it" }
                }
            }
            if (!exists) Flat("Restore", color = Tc.dim) { pick.launch(arrayOf("*/*")) }
        }
    }
}
