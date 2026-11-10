package com.torrbaby.tv;

import android.content.*;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.*;
import android.view.*;
import android.widget.*;

final class Ui {
  private static final Typeface BODY_FONT = Typeface.create("sans-serif", Typeface.NORMAL);
  private static final Typeface LABEL_FONT = Typeface.create("sans-serif-medium", Typeface.NORMAL);
  private static final Typeface TITLE_FONT = Typeface.create("sans-serif", Typeface.BOLD);
  static final int BG = Color.BLACK,
      PANEL = Color.rgb(20,20,20),
      TEXT = Color.WHITE,
      MUTED = Color.rgb(170,170,170),
      ACCENT = Color.WHITE;

  static int dp(Context c, float n) {
    return (int) (n * c.getResources().getDisplayMetrics().density + .5f);
  }

  static GradientDrawable shape(int color, int stroke, Context c) {
    GradientDrawable g = new GradientDrawable();
    g.setColor(color);
    g.setCornerRadius(dp(c, 10));
    if (stroke != 0) g.setStroke(dp(c, 2), stroke);
    return g;
  }

  static void focus(View v) {
    Context c = v.getContext();
    StateListDrawable s = new StateListDrawable();
    s.addState(new int[] {android.R.attr.state_focused}, shape(v instanceof TextView ? Color.WHITE : Color.TRANSPARENT, Color.WHITE, c));
    s.addState(new int[] {android.R.attr.state_pressed}, shape(v instanceof TextView ? Color.WHITE : Color.TRANSPARENT, Color.WHITE, c));
    s.addState(new int[] {}, shape(Color.TRANSPARENT, 0, c));
    v.setBackground(s);
    if(v instanceof TextView)((TextView)v).setTextColor(new android.content.res.ColorStateList(
        new int[][]{new int[]{android.R.attr.state_focused},new int[]{android.R.attr.state_pressed},new int[]{}},
        new int[]{Color.BLACK,Color.BLACK,Color.WHITE}));
    v.setFocusable(true);
    v.setFocusableInTouchMode(true);
  }

  static void primary(Button b) {
    focus(b);
  }

  static TextView text(Context c, String s, int size, int color) {
    TextView t = new TextView(c);
    t.setText(s);
    t.setTextSize(size);
    t.setTextColor(color);
    t.setIncludeFontPadding(false);
    t.setTypeface(BODY_FONT);
    t.setLetterSpacing(0);
    return t;
  }

  static TextView title(Context c, String s, int size) {
    TextView t = text(c, s, size, TEXT);
    t.setTypeface(size >= 24 ? TITLE_FONT : LABEL_FONT);
    t.setLetterSpacing(size >= 24 ? -.025f : -.01f);
    return t;
  }

  static Button button(Context c, String label, Runnable run) {
    Button b = new Button(c);
    b.setText(label);
    b.setTextSize(15);
    b.setTypeface(LABEL_FONT);
    b.setLetterSpacing(0);
    b.setIncludeFontPadding(false);
    b.setStateListAnimator(null);

    b.setAllCaps(false);
    b.setMinHeight(0);
    b.setMinimumHeight(0);
    b.setPadding(dp(c, 18), dp(c, 8), dp(c, 18), dp(c, 8));
    focus(b);
    b.setOnClickListener(v -> run.run());
    return b;
  }

  static Button iconButton(Context c, String label, int icon, Runnable run) {
    Button b=button(c,label,run);
    b.setCompoundDrawablesRelativeWithIntrinsicBounds(icon,0,0,0);
    b.setCompoundDrawablePadding(dp(c,10));
    b.setCompoundDrawableTintList(b.getTextColors());
    return b;
  }

  static void secondaryContrast(View v, boolean focused) {
    if (v instanceof TextView) {
      TextView text = (TextView) v;
      if (text.getCurrentTextColor() == MUTED || text.getCurrentTextColor() == 0xffe2e7ea)
        text.setTextColor(focused ? 0xffe2e7ea : MUTED);
    }
    if (v instanceof android.view.ViewGroup) {
      android.view.ViewGroup group = (android.view.ViewGroup) v;
      for (int i = 0; i < group.getChildCount(); i++)
        secondaryContrast(group.getChildAt(i), focused);
    }
  }

  static void tab(Button b, boolean selected) {
    StateListDrawable states = new StateListDrawable();
    GradientDrawable focused = shape(TEXT, 0, b.getContext());
    focused.setCornerRadius(dp(b.getContext(), 8));
    states.addState(new int[] {android.R.attr.state_focused}, focused);
    states.addState(new int[] {}, shape(selected ? Color.WHITE : Color.TRANSPARENT, 0, b.getContext()));
    b.setBackground(states);
    b.setTextColor(
        new android.content.res.ColorStateList(
            new int[][] {new int[] {android.R.attr.state_focused}, new int[] {}},
            new int[] {BG, selected ? Color.BLACK : Color.WHITE}));
  }

  static void round(View v, int radius) {
    v.setOutlineProvider(
        new ViewOutlineProvider() {
          public void getOutline(View view, android.graphics.Outline outline) {
            outline.setRoundRect(
                0, 0, view.getWidth(), view.getHeight(), dp(view.getContext(), radius));
          }
        });
    v.setClipToOutline(true);
  }

  static LinearLayout column(Context c) {
    LinearLayout l = new LinearLayout(c);
    l.setOrientation(LinearLayout.VERTICAL);
    return l;
  }

  static LinearLayout row(Context c) {
    LinearLayout l = new LinearLayout(c);
    l.setOrientation(LinearLayout.HORIZONTAL);
    l.setGravity(Gravity.CENTER_VERTICAL);
    return l;
  }

  static void add(LinearLayout l, View v, int width, int height) {
    l.addView(
        v,
        new LinearLayout.LayoutParams(
            width < 0 ? width : dp(l.getContext(), width),
            height < 0 ? height : dp(l.getContext(), height)));
  }

  static void gap(LinearLayout l, int size) {
    View v = new View(l.getContext());
    add(
        l,
        v,
        l.getOrientation() == LinearLayout.HORIZONTAL ? size : 1,
        l.getOrientation() == LinearLayout.VERTICAL ? size : 1);
  }

  static void pad(View v, int h, int vertical) {
    v.setPadding(
        dp(v.getContext(), h),
        dp(v.getContext(), vertical),
        dp(v.getContext(), h),
        dp(v.getContext(), vertical));
  }
}
