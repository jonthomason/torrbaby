package com.torrbaby.tv

import android.content.BroadcastReceiver
import android.content.Context
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge

/**
 * The phone and tablet app: the iPhone app's screens (PhoneScreen.kt) over the TV app's connection,
 * library and player code (PhoneModel.kt). TVs keep MainActivity's screens, made for a remote.
 */
class PhoneActivity : ComponentActivity() {
    private var updateStatus: BroadcastReceiver? = null

    override fun onCreate(saved: Bundle?) {
        enableEdgeToEdge(SystemBarStyle.dark(Color.TRANSPARENT), SystemBarStyle.dark(Color.TRANSPARENT))
        super.onCreate(saved)
        window.setBackgroundDrawable(android.graphics.drawable.ColorDrawable(Color.BLACK))
        Phone.attach(this)
        // Android's installer answers an update here: its confirmation to show, or why it failed.
        updateStatus = Updates.receiver(this) { Phone.problem = it }.also {
            if (Build.VERSION.SDK_INT >= 33) registerReceiver(it, Updates.filter(), Context.RECEIVER_NOT_EXPORTED)
            else registerReceiver(it, Updates.filter())
        }
        setContent { PhoneScreen(this) }
    }

    override fun onResume() {
        super.onResume()
        // Back from the player or another app: watched marks and new titles, and any newer build.
        if (Phone.stage == Stage.Ready) Phone.refresh()
    }

    override fun onDestroy() {
        updateStatus?.let { unregisterReceiver(it) }
        super.onDestroy()
    }
}
