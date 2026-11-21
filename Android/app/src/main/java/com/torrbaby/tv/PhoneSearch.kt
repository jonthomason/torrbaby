package com.torrbaby.tv

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * A search box as on the iPhone: the magnifier, the words, and ✕ to clear them while there
 * are any. The keyboard's own key (Search, or Done for a filter) acts and puts the keyboard away.
 */
@Composable
internal fun SearchField(value: String, onValue: (String) -> Unit, placeholder: String, action: ImeAction = ImeAction.Done,
                         modifier: Modifier = Modifier, onAction: () -> Unit = {}, onClear: () -> Unit = { onValue("") }) {
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    val act = { focus.clearFocus(); keyboard?.hide(); onAction() }
    Row(modifier.fillMaxWidth().height(42.dp).clip(RoundedCornerShape(10.dp)).background(Ink.control).padding(start = 10.dp),
        verticalAlignment = Alignment.CenterVertically) {
        Symbol(Glyph.Search, 18.dp, Ink.muted)
        Spacer(Modifier.width(8.dp))
        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
            if (value.isEmpty()) Words(placeholder, color = Ink.muted, lines = 1)
            BasicTextField(value, onValue, Modifier.fillMaxWidth(), singleLine = true,
                textStyle = TextStyle(color = Ink.text, fontSize = 16.sp), cursorBrush = SolidColor(Ink.text),
                keyboardOptions = KeyboardOptions(imeAction = action),
                keyboardActions = KeyboardActions(onSearch = { act() }, onDone = { act() }, onGo = { act() }))
        }
        if (value.isNotEmpty()) {
            val label = L10n.t("Clear search")
            Box(Modifier.size(42.dp).clip(CircleShape).clickable(onClick = onClear).semantics { contentDescription = label },
                contentAlignment = Alignment.Center) { Symbol(Glyph.Clear, 18.dp, Ink.muted) }
        } else Spacer(Modifier.width(10.dp))
    }
}

/** Scrolling the list under a search box puts the keyboard away, as on the iPhone. */
@Composable
internal fun Modifier.dismissesKeyboard(): Modifier {
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    val connection = remember(focus, keyboard) {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (source == NestedScrollSource.UserInput && available.y != 0f) { focus.clearFocus(); keyboard?.hide() }
                return Offset.Zero
            }
        }
    }
    return nestedScroll(connection)
}
