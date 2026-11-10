package com.torrbaby.tv;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Matrix;
import android.graphics.drawable.Drawable;
import android.widget.ImageView;

/** Fill the hero while retaining the top of the artwork, where faces usually sit. */
final class HeroImageView extends ImageView {
  private final Matrix crop = new Matrix();
  HeroImageView(Context context) { super(context);setScaleType(ScaleType.MATRIX); }
  @Override protected void onDraw(Canvas canvas) {
    Drawable image=getDrawable();
    if(image!=null && image.getIntrinsicWidth()>0 && image.getIntrinsicHeight()>0) {
      float scale=Math.max((float)getWidth()/image.getIntrinsicWidth(),(float)getHeight()/image.getIntrinsicHeight());
      crop.setScale(scale,scale);
      crop.postTranslate((getWidth()-image.getIntrinsicWidth()*scale)/2,0);
      setImageMatrix(crop);
    }
    super.onDraw(canvas);
  }
}
