package com.torrbaby.tv;

import android.app.AlertDialog;
import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.widget.AbsListView;
import android.widget.Button;
import android.widget.CheckedTextView;
import android.widget.EditText;
import android.widget.TextView;

/**
 * Every dialog in the app: a dark panel with light text and the app's own buttons.
 *
 * A dialog theme is not enough. Some televisions replace the system dialog with their own
 * (the JMGO projector's is a white panel with blue buttons) and keep the app's light list
 * text, so a list such as Settings was white on white. Here the dialog is painted after it
 * is laid out, whatever the television drew: the window gets the app's panel, the
 * television's own panel backgrounds are removed, dark text is made light, and buttons take
 * the app's focus style. Views the app put in itself (a profile editor) are left alone.
 */
final class AppDialog extends AlertDialog.Builder {
  AppDialog(Context context) { super(context); }

  @Override public AlertDialog create() {
    AlertDialog dialog = super.create();
    Window window = dialog.getWindow();
    if (window != null) {
      window.setBackgroundDrawableResource(R.drawable.dialog_background);
      View decor = window.getDecorView();
      // On every layout: list rows are made as they scroll into view.
      decor.getViewTreeObserver().addOnGlobalLayoutListener(() -> paint(decor, true));
    }
    return dialog;
  }

  private static void paint(View view, boolean root) {
    if (view.getId() == android.R.id.custom) return;
    if (view instanceof Button) {
      Button button = (Button) view;
      if (button.getTag(R.id.dialog_painted) == null) {
        button.setTag(R.id.dialog_painted, true);
        button.setAllCaps(false);
        button.setStateListAnimator(null);
        Ui.focus(button);
      }
      return;
    }
    if (view instanceof EditText) {
      ((EditText) view).setTextColor(Color.WHITE);
      ((EditText) view).setHintTextColor(Ui.MUTED);
      return;
    }
    if (view instanceof TextView) {
      TextView text = (TextView) view;
      int color = text.getCurrentTextColor();
      // Dark text was meant for a light panel.
      if (Color.red(color) * 299 + Color.green(color) * 587 + Color.blue(color) * 114 < 128_000) text.setTextColor(Color.WHITE);
      if (text instanceof CheckedTextView) ((CheckedTextView) text).setCheckMarkTintList(ColorStateList.valueOf(Color.WHITE));
      return;
    }
    if (!root && view.getBackground() != null && view.getTag(R.id.dialog_painted) == null) {
      view.setTag(R.id.dialog_painted, true);
      view.setBackground(null);
    }
    if (view instanceof AbsListView && view.getTag(R.id.dialog_selector) == null) {
      view.setTag(R.id.dialog_selector, true);
      ((AbsListView) view).setSelector(Ui.shape(Color.argb(70, 255, 255, 255), 0, view.getContext()));
    }
    if (view instanceof ViewGroup) {
      ViewGroup group = (ViewGroup) view;
      for (int i = 0; i < group.getChildCount(); i++) paint(group.getChildAt(i), false);
    }
  }
}
