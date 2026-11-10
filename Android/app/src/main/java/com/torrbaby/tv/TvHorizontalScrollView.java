package com.torrbaby.tv;

import android.content.Context;
import android.graphics.Rect;
import android.view.FocusFinder;
import android.view.KeyEvent;
import android.view.View;
import android.widget.HorizontalScrollView;

/** D-pad navigation glides the row to the focused item instead of snapping to it. */
final class TvHorizontalScrollView extends HorizontalScrollView {
  private android.animation.ValueAnimator glide;
  private boolean focusing;

  TvHorizontalScrollView(Context context) { super(context); }

  @Override public void requestChildFocus(View child, View focused) {
    focusing=true;
    try { super.requestChildFocus(child,focused); } finally { focusing=false; }
  }

  // Focus styling often requests a layout before the focus change reaches this view; the
  // platform then reveals the item during layout, so those scrolls are animated as well.
  @Override protected void onLayout(boolean changed,int left,int top,int right,int bottom) {
    focusing=isLaidOut();
    try { super.onLayout(changed,left,top,right,bottom); } finally { focusing=false; }
  }

  @Override public void scrollTo(int x,int y) {
    if(!focusing || !isLaidOut()) { super.scrollTo(x,y); return; }
    if(x!=getScrollX())animateTo(x);
    else if(glide==null || !glide.isRunning())super.scrollTo(x,y);
  }

  @Override public boolean requestChildRectangleOnScreen(View child, Rect rectangle, boolean immediate) {
    Rect target=new Rect(rectangle);
    offsetDescendantRectToMyCoords(child,target);
    int delta=computeScrollDeltaToGetChildRectOnScreen(target);
    if(delta==0)return false;
    animateTo(getScrollX()+delta);
    return true;
  }

  private void animateTo(int target) {
    int max=getChildCount()==0?0:Math.max(0,getChildAt(0).getWidth()-(getWidth()-getPaddingLeft()-getPaddingRight()));
    target=Math.max(0,Math.min(max,target));
    if(glide!=null)glide.cancel();
    if(target==getScrollX())return;
    glide=android.animation.ValueAnimator.ofInt(getScrollX(),target);
    glide.setDuration(260);
    glide.setInterpolator(new android.view.animation.DecelerateInterpolator());
    glide.addUpdateListener(value->super.scrollTo((Integer)value.getAnimatedValue(),0));
    glide.start();
  }

  @Override protected void onDetachedFromWindow() {
    if(glide!=null)glide.cancel();
    super.onDetachedFromWindow();
  }

  @Override public boolean executeKeyEvent(KeyEvent event) {
    if (event.getAction() == KeyEvent.ACTION_DOWN
        && (event.getKeyCode() == KeyEvent.KEYCODE_DPAD_LEFT
            || event.getKeyCode() == KeyEvent.KEYCODE_DPAD_RIGHT)) {
      int direction = event.getKeyCode() == KeyEvent.KEYCODE_DPAD_RIGHT
          ? View.FOCUS_RIGHT : View.FOCUS_LEFT;
      View current = findFocus();
      if (current != null && current != this) {
        // The stock handler starts its own scroll and then snaps to the focused child.
        View next = FocusFinder.getInstance().findNextFocus(this, current, direction);
        if (next != null && next != current && next.requestFocus(direction)) return true;
      }
    }
    return super.executeKeyEvent(event);
  }
}
