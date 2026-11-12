package com.torrbaby.tv;

import android.app.AlertDialog;
import android.content.Context;
import android.content.DialogInterface;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import java.util.ArrayList;
import java.util.List;
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
 *
 * A dialog that offers a list of choices draws the whole panel itself: title, one row per
 * choice and its buttons. The JMGO's own dialog showed no rows at all for a system list,
 * only the title and Cancel, so a menu such as More could not be used.
 */
final class AppDialog extends AlertDialog.Builder {
  private CharSequence title;
  private CharSequence[] items;
  private int checked = -1;
  private boolean single;
  private DialogInterface.OnClickListener choose;
  private final List<Object[]> buttons = new ArrayList<>();

  AppDialog(Context context) { super(context); }

  @Override public AppDialog setTitle(CharSequence title) { this.title = title; return this; }

  @Override public AppDialog setItems(CharSequence[] items, DialogInterface.OnClickListener listener) {
    this.items = items; single = false; choose = listener; return this;
  }

  @Override public AppDialog setSingleChoiceItems(CharSequence[] items, int checked, DialogInterface.OnClickListener listener) {
    this.items = items; this.checked = checked; single = true; choose = listener; return this;
  }

  @Override public AppDialog setPositiveButton(CharSequence text, DialogInterface.OnClickListener listener) {
    buttons.add(new Object[] {DialogInterface.BUTTON_POSITIVE, text, listener}); return this;
  }

  @Override public AppDialog setNegativeButton(CharSequence text, DialogInterface.OnClickListener listener) {
    buttons.add(new Object[] {DialogInterface.BUTTON_NEGATIVE, text, listener}); return this;
  }

  @Override public AppDialog setNeutralButton(CharSequence text, DialogInterface.OnClickListener listener) {
    buttons.add(new Object[] {DialogInterface.BUTTON_NEUTRAL, text, listener}); return this;
  }

  @Override public AlertDialog create() {
    AlertDialog[] made = new AlertDialog[1];
    if (items != null) {
      super.setView(choices(made));
    } else {
      super.setTitle(title);
      for (Object[] b : buttons) {
        DialogInterface.OnClickListener l = (DialogInterface.OnClickListener) b[2];
        switch ((int) b[0]) {
          case DialogInterface.BUTTON_POSITIVE: super.setPositiveButton((CharSequence) b[1], l); break;
          case DialogInterface.BUTTON_NEGATIVE: super.setNegativeButton((CharSequence) b[1], l); break;
          default: super.setNeutralButton((CharSequence) b[1], l);
        }
      }
    }
    AlertDialog dialog = super.create();
    made[0] = dialog;
    Window window = dialog.getWindow();
    if (window != null) {
      // A list draws its own panel; the window behind it stays clear.
      if (items != null) window.setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(Color.TRANSPARENT));
      else window.setBackgroundDrawableResource(R.drawable.dialog_background);
      View decor = window.getDecorView();
      // On every layout: list rows are made as they scroll into view.
      decor.getViewTreeObserver().addOnGlobalLayoutListener(() -> paint(decor, true));
    }
    return dialog;
  }

  /** The title, a row per choice (the chosen one marked) and the buttons, on the app's panel. */
  private View choices(AlertDialog[] made) {
    Context c = getContext();
    LinearLayout panel = new LinearLayout(c);
    panel.setOrientation(LinearLayout.VERTICAL);
    int pad = Ui.dp(c, 24);
    panel.setPadding(pad, pad, pad, Ui.dp(c, 16));
    GradientDrawable background = new GradientDrawable();
    background.setColor(Color.rgb(0x1C, 0x1C, 0x1E));
    background.setCornerRadius(Ui.dp(c, 24));
    background.setStroke(Ui.dp(c, 1), Color.rgb(0x3A, 0x3A, 0x3C));
    panel.setBackground(background);
    if (title != null && title.length() > 0) {
      TextView heading = Ui.title(c, title.toString(), 22);
      heading.setPadding(Ui.dp(c, 18), 0, Ui.dp(c, 18), Ui.dp(c, 14));
      panel.addView(heading, new LinearLayout.LayoutParams(-1, -2));
    }
    LinearLayout rows = new LinearLayout(c);
    rows.setOrientation(LinearLayout.VERTICAL);
    Button first = null;
    for (int i = 0; i < items.length; i++) {
      int which = i;
      String label = (single ? (i == checked ? "✓  " : "     ") : "") + items[i];
      Button row = Ui.button(c, label, () -> {
        AlertDialog dialog = made[0];
        if (dialog == null) return;
        if (choose != null) choose.onClick(dialog, which);
        if (!single) dialog.dismiss();
      });
      row.setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
      row.setTextSize(17);
      LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(-1, Ui.dp(c, 48));
      lp.bottomMargin = Ui.dp(c, 4);
      rows.addView(row, lp);
      if (first == null || i == checked) first = row;
    }
    ScrollView scroll = new ScrollView(c);
    scroll.addView(rows);
    // Long lists (genres) scroll within about eight rows.
    panel.addView(scroll, new LinearLayout.LayoutParams(-1, items.length > 8 ? Ui.dp(c, 8 * 52) : -2));
    if (!buttons.isEmpty()) {
      LinearLayout bar = new LinearLayout(c);
      bar.setGravity(Gravity.END);
      bar.setPadding(0, Ui.dp(c, 12), 0, 0);
      for (Object[] b : buttons) {
        int which = (int) b[0];
        DialogInterface.OnClickListener l = (DialogInterface.OnClickListener) b[2];
        Button button = Ui.button(c, b[1].toString(), () -> {
          AlertDialog dialog = made[0];
          if (dialog == null) return;
          if (l != null) l.onClick(dialog, which);
          dialog.dismiss();
        });
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(-2, Ui.dp(c, 44));
        lp.leftMargin = Ui.dp(c, 8);
        bar.addView(button, lp);
      }
      panel.addView(bar, new LinearLayout.LayoutParams(-1, -2));
    }
    if (first != null) { Button focus = first; panel.post(focus::requestFocus); }
    return panel;
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
