package app.termchat

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * Keeps the process, and with it the unlocked account's relay connection,
 * alive in the background (ADR-012, section 1). Its notification says only
 * that termchat is connected.
 */
class ConnectionService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val note = notification(this, CHANNEL_LINK, "termchat.app is connected", silent = true)
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(ID_LINK, note, ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING)
        } else {
            startForeground(ID_LINK, note)
        }
        return START_NOT_STICKY
    }

    companion object {
        fun start(context: Context) = context.startForegroundService(Intent(context, ConnectionService::class.java))
        fun stop(context: Context) = context.stopService(Intent(context, ConnectionService::class.java))
    }
}

const val CHANNEL_LINK = "link"
const val CHANNEL_MESSAGES = "messages"
const val ID_LINK = 1
const val ID_MESSAGE = 2

fun channels(context: Context) {
    val nm = context.getSystemService(NotificationManager::class.java)
    nm.createNotificationChannel(NotificationChannel(CHANNEL_LINK, "Connection", NotificationManager.IMPORTANCE_MIN))
    nm.createNotificationChannel(NotificationChannel(CHANNEL_MESSAGES, "New messages", NotificationManager.IMPORTANCE_DEFAULT))
}

/** No names, no text, no counts: only that something arrived (ADR-008). */
fun notification(context: Context, channel: String, text: String, silent: Boolean = false): Notification {
    val open = PendingIntent.getActivity(
        context, 0, Intent(context, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
    )
    return Notification.Builder(context, channel)
        .setSmallIcon(R.drawable.ic_notification)
        .setContentTitle("termchat")
        .setContentText(text)
        .setContentIntent(open)
        .setAutoCancel(!silent)
        .setOngoing(silent)
        .setVisibility(Notification.VISIBILITY_SECRET)
        .build()
}
