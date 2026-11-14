package app.termchat

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * ADR-022: Android freezes a backgrounded app, and a frozen one runs no idle
 * check. Without "stay connected" this alarm wakes it when the lock time is
 * up, and the usual check locks it. Inexact: it may come some minutes late.
 */
class LockAlarm : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        (context.applicationContext as TermchatApp).model.lockIfIdle()
    }

    companion object {
        private fun pending(context: Context) = PendingIntent.getBroadcast(
            context, 0, Intent(context, LockAlarm::class.java), PendingIntent.FLAG_IMMUTABLE,
        )

        fun arm(context: Context, at: Long) = context.getSystemService(AlarmManager::class.java)
            .setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending(context))

        fun disarm(context: Context) = context.getSystemService(AlarmManager::class.java).cancel(pending(context))
    }
}
