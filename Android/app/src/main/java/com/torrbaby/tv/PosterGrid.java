package com.torrbaby.tv;

import android.content.Context;
import android.view.KeyEvent;
import android.view.View;
import android.widget.GridView;

/** Remote navigation moves between whole rows and retains the selected column. */
final class PosterGrid extends GridView {
  PosterGrid(Context context) { super(context); }
  private boolean confirm(int code) { return code==KeyEvent.KEYCODE_DPAD_CENTER || code==KeyEvent.KEYCODE_ENTER || code==KeyEvent.KEYCODE_NUMPAD_ENTER; }
  @Override public boolean onKeyUp(int code,KeyEvent event) {
    if(confirm(code)) {
      View selected=getSelectedView(); int position=getSelectedItemPosition();
      if(!event.isCanceled() && selected!=null && position>=0)performItemClick(selected,position,getAdapter().getItemId(position));
      return true;
    }
    return super.onKeyUp(code,event);
  }
  @Override public boolean onKeyDown(int keyCode, KeyEvent event) {
    if(confirm(keyCode))return true;
    if(keyCode==KeyEvent.KEYCODE_DPAD_DOWN || keyCode==KeyEvent.KEYCODE_DPAD_UP) {
      int selected=getSelectedItemPosition(),columns=getNumColumns();
      if(selected>=0 && columns>0) {
        int direction=keyCode==KeyEvent.KEYCODE_DPAD_DOWN?1:-1;
        int stride=event.getRepeatCount()>=8?2:1;
        if(direction<0 && selected<columns)return super.onKeyDown(keyCode,event);
        int next=Math.max(0,Math.min(getCount()-1,selected+direction*columns*stride));
        if(next!=selected) {
          View current=getSelectedView();
          int height=current==null?getHeight()/2:current.getHeight();
          int offset=direction>0?Math.max(0,getHeight()-getPaddingTop()-getPaddingBottom()-height):0;
          setSelectionFromTop(next,offset);
        }
        return true;
      }
    }
    boolean handled=super.onKeyDown(keyCode,event);
    if(handled && (keyCode==KeyEvent.KEYCODE_DPAD_LEFT || keyCode==KeyEvent.KEYCODE_DPAD_RIGHT))requestLayout();
    return handled;
  }
  private boolean revealing;
  @Override protected void onLayout(boolean changed,int left,int top,int right,int bottom) {
    super.onLayout(changed,left,top,right,bottom);
    if(!revealing) {
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
