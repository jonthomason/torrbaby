package com.torrbaby.tv;

import android.content.Context;
import android.graphics.drawable.Drawable;
import android.widget.ImageView;

/**
 * Artwork whose size comes from its layout, never from the image. A plain ImageView asks the
 * whole window to lay out again whenever a new image arrives; on the home page that is every
 * shelf, once per poster, which made the first scroll down stutter while posters loaded.
 */
class ArtImageView extends ImageView {
  private boolean assigning;

  ArtImageView(Context context) {
    super(context);
  }

  @Override
  public void setImageDrawable(Drawable drawable) {
    assigning = true;
    try {
      super.setImageDrawable(drawable);
    } finally {
      assigning = false;
    }
  }

  @Override
  public void requestLayout() {
    // The new image is drawn by the invalidate that follows; the size stays as laid out.
    if (!assigning) super.requestLayout();
  }
}
