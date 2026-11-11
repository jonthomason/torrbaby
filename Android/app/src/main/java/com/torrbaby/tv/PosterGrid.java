package com.torrbaby.tv;

import android.content.Context;
import android.view.KeyEvent;
import android.view.View;
import android.widget.GridView;

/** Remote navigation moves between whole rows and retains the selected column. */
final class PosterGrid extends GridView {
  private static final int GLIDE_MS=240;
  // A single row step scrolls into place first; the selection is committed when it lands.
  private int pending=-1,pendingOffset;
  private final Runnable land=this::commit;
  PosterGrid(Context context) {
    super(context);
    // The focused poster grows past its cell. Grid cells clip by default, and while a row
    // glides the grid draws cells from cached bitmaps on older TVs, which clips them too.
    setClipChildren(false);
    setScrollingCacheEnabled(false);
    setAnimationCacheEnabled(false);
  }
  private boolean confirm(int code) { return code==KeyEvent.KEYCODE_DPAD_CENTER || code==KeyEvent.KEYCODE_ENTER || code==KeyEvent.KEYCODE_NUMPAD_ENTER; }

  /** The position shown as focused, including a row step that is still gliding. */
  int highlighted() { return pending>=0?pending:getSelectedItemPosition(); }

  private void commit() {
    if(pending<0)return;
    int position=pending; pending=-1;
    removeCallbacks(land);
    smoothScrollBy(0,0);
    setSelectionFromTop(position,pendingOffset);
    layoutChildren();
  }

  private void highlight() {
    int first=getFirstVisiblePosition(),shown=highlighted();
    for(int i=0;i<getChildCount();i++) {
      View child=getChildAt(i);
      if(child instanceof PosterCard)((PosterCard)child).focusAppearance(first+i==shown);
    }
  }

  @Override public boolean onKeyUp(int code,KeyEvent event) {
    if(confirm(code)) {
      commit();
      View selected=getSelectedView(); int position=getSelectedItemPosition();
      if(!event.isCanceled() && selected!=null && position>=0)performItemClick(selected,position,getAdapter().getItemId(position));
      return true;
    }
    return super.onKeyUp(code,event);
  }
  @Override public boolean onKeyDown(int keyCode, KeyEvent event) {
    commit();
    if(confirm(keyCode))return true;
    if(keyCode==KeyEvent.KEYCODE_DPAD_DOWN || keyCode==KeyEvent.KEYCODE_DPAD_UP) {
      int selected=getSelectedItemPosition(),columns=getNumColumns();
      if(selected>=0 && columns>0) {
        int direction=keyCode==KeyEvent.KEYCODE_DPAD_DOWN?1:-1;
        int stride=event.getRepeatCount()>=8?2:1;
        if(direction<0 && selected<columns)return super.onKeyDown(keyCode,event);
        int next=Math.max(0,Math.min(getCount()-1,selected+direction*columns*stride));
        if(next!=selected) {
          View current=getChildAt(selected-getFirstVisiblePosition());
          int height=current==null?getHeight()/2:current.getHeight();
          int offset=direction>0?Math.max(0,getHeight()-getPaddingTop()-getPaddingBottom()-height):0;
          // Held keys keep their immediate traversal; a deliberate press glides one row.
          if(current!=null && event.getRepeatCount()==0 && next/columns==selected/columns+direction) {
            View target=getChildAt(next-getFirstVisiblePosition());
            int top=target!=null?target.getTop():current.getTop()+direction*(height+getVerticalSpacing());
            int distance=top-(getPaddingTop()+offset);
            if(distance*direction>0) {
              pending=next; pendingOffset=offset;
              highlight();
              smoothScrollBy(distance,GLIDE_MS);
              postDelayed(land,GLIDE_MS+40);
              return true;
            }
          }
          setSelectionFromTop(next,offset);
        }
        return true;
      }
    }
    boolean handled=super.onKeyDown(keyCode,event);
    if(handled && (keyCode==KeyEvent.KEYCODE_DPAD_LEFT || keyCode==KeyEvent.KEYCODE_DPAD_RIGHT))requestLayout();
    return handled;
  }
  @Override protected void onDetachedFromWindow() {
    removeCallbacks(land); pending=-1;
    super.onDetachedFromWindow();
  }
  private boolean revealing;
  @Override protected void onLayout(boolean changed,int left,int top,int right,int bottom) {
    super.onLayout(changed,left,top,right,bottom);
    if(!revealing && pending<0) {
      revealing=true;
      revealSelection();
      revealing=false;
    }
  }
  private void revealSelection() {
    View selected=getSelectedView();
    if(selected==null)return;
    int top=getPaddingTop(),bottom=getHeight()-getPaddingBottom();
    if(selected.getTop()<top || selected.getBottom()>bottom) {
      int position=getSelectedItemPosition();
      post(() -> { if(getSelectedItemPosition()==position)setSelectionFromTop(position,0); });
    }
  }
}
