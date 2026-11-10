package com.torrbaby.tv;

import android.content.Context;
import android.graphics.Rect;
import android.view.FocusFinder;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewParent;
import android.widget.HorizontalScrollView;
import android.widget.ScrollView;

/** D-pad navigation reveals a complete focused row, using the actual content viewport. */
final class TvScrollView extends ScrollView {
  private android.animation.ValueAnimator rowScroll;
  private boolean focusing;

  @Override public void requestChildFocus(View child, View focused) {
    focusing=true;
    try { super.requestChildFocus(child,focused); } finally { focusing=false; }
  }

  @Override public void scrollTo(int x,int y) {
    if(focusing && isLaidOut())animateRow(y); else super.scrollTo(x,y);
  }

  @Override public boolean requestChildRectangleOnScreen(View child, Rect rectangle, boolean immediate) {
    Rect target=new Rect(rectangle);
    offsetDescendantRectToMyCoords(child,target);
    int delta=computeScrollDeltaToGetChildRectOnScreen(target);
    if(delta==0)return false;
    animateRow(getScrollY()+delta);
    return true;
  }

  private void animateRow(int target) {
    if(rowScroll!=null)rowScroll.cancel();
    if(target==getScrollY())return;
    rowScroll=android.animation.ValueAnimator.ofInt(getScrollY(),target);
    rowScroll.setDuration(280);
    rowScroll.setInterpolator(new android.view.animation.DecelerateInterpolator());
    rowScroll.addUpdateListener(value->super.scrollTo(0,(Integer)value.getAnimatedValue()));
    rowScroll.start();
  }

  @Override protected void onDetachedFromWindow() {
    if(rowScroll!=null)rowScroll.cancel();
    super.onDetachedFromWindow();
  }

  TvScrollView(Context context) {
    super(context);
    setSmoothScrollingEnabled(true);
  }

  @Override public boolean executeKeyEvent(KeyEvent event) {
    if (event.getAction() == KeyEvent.ACTION_DOWN && !event.isAltPressed()
        && (event.getKeyCode() == KeyEvent.KEYCODE_DPAD_DOWN
            || event.getKeyCode() == KeyEvent.KEYCODE_DPAD_UP)) {
      int direction = event.getKeyCode() == KeyEvent.KEYCODE_DPAD_DOWN
          ? View.FOCUS_DOWN : View.FOCUS_UP;
      View current = findFocus();
      if (current != null && current != this) {
        View next = FocusFinder.getInstance().findNextFocus(this, current, direction);
        // ScrollView's default arrow handler limits each jump to half a viewport,
        // which can leave focus on the old row while only part of the next is visible.
        if (next != null && next != current && next.requestFocus(direction)) return true;
      }
    }
    return super.executeKeyEvent(event);
  }

  @Override protected int computeScrollDeltaToGetChildRectOnScreen(Rect rect) {
    if (getChildCount() == 0 || getHeight() == 0) return 0;
    Rect target = new Rect(rect);
    View focused = findFocus();
    // Include the full poster shelf, not just the small focused heading/button.
    if (focused != null && focused != this) {
      View row = focused;
      for (ViewParent p = focused.getParent(); p != null && p != this; p = p.getParent()) {
        if (p instanceof HorizontalScrollView) { row = (View) p; break; }
      }
      row.getDrawingRect(target);
      offsetDescendantRectToMyCoords(row, target);
    }
    int margin = Ui.dp(getContext(), 12);
    int top = getScrollY() + getPaddingTop() + margin;
    int bottom = getScrollY() + getHeight() - getPaddingBottom() - margin;
    int delta = 0;
    if (target.height() > bottom - top) delta = target.top - top;
    else if (target.top < top) delta = target.top - top;
    else if (target.bottom > bottom) delta = target.bottom - bottom;
    int max = Math.max(0, getChildAt(0).getBottom() + getPaddingBottom() - getHeight());
    return Math.max(0, Math.min(max, getScrollY() + delta)) - getScrollY();
  }
}
