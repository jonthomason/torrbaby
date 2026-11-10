package com.torrbaby.tv;

import android.content.Context;
import android.graphics.Color;
import android.text.TextUtils;
import android.view.*;
import android.widget.*;
import java.util.function.Consumer;

/** A reusable poster: binding never reconstructs its view hierarchy. */
final class PosterCard extends LinearLayout {
  static int created;
  private final ImageView poster;
  private final FrameLayout artwork;
  private final TextView title, subtitle, watched, remaining, year;
  private boolean continuing;
  private final ProgressBar progress;
  private final Art art;
  private Library.Title item;

  PosterCard(Context context, Art art, Consumer<Library.Title> open, Consumer<Library.Title> focused) {
    super(context);
    created++;
    this.art = art;
    setOrientation(VERTICAL);
    setFocusable(true);
    setFocusableInTouchMode(true);
    setClipChildren(false);
    artwork = new FrameLayout(context);
    poster = new ImageView(context);
    poster.setScaleType(ImageView.ScaleType.CENTER_CROP);
    poster.setBackgroundColor(Ui.PANEL);
    artwork.addView(poster, new FrameLayout.LayoutParams(-1, -1));
    watched = Ui.text(context, "✓", 13, Color.WHITE);
    watched.setGravity(Gravity.CENTER);
    watched.setBackground(Ui.shape(0xB0000000, 0, context));
    watched.setContentDescription(L10n.t("Watched"));
    FrameLayout.LayoutParams badge = new FrameLayout.LayoutParams(Ui.dp(context,25),Ui.dp(context,25),Gravity.TOP|Gravity.END);
    badge.setMargins(0,Ui.dp(context,6),Ui.dp(context,6),0);
    artwork.addView(watched,badge);
    Ui.round(artwork,10);
    addView(artwork,new LinearLayout.LayoutParams(-1,Ui.dp(context,180)));
    Ui.gap(this,10);
    title=Ui.title(context,"",14); title.setMaxLines(3); title.setEllipsize(TextUtils.TruncateAt.END);
    title.setGravity(Gravity.CENTER);
    artwork.addView(title,new FrameLayout.LayoutParams(-1,-1));
    subtitle=Ui.text(context,"",11,Ui.MUTED); subtitle.setSingleLine(true); subtitle.setEllipsize(TextUtils.TruncateAt.END);
    Ui.add(this,subtitle,-1,17);
    remaining=Ui.text(context,"",11,Ui.MUTED);remaining.setSingleLine(true);Ui.add(this,remaining,-1,17);remaining.setVisibility(GONE);
    year=Ui.text(context,"",11,Ui.MUTED);year.setSingleLine(true);Ui.add(this,year,-1,17);year.setVisibility(GONE);
    subtitle.setGravity(Gravity.CENTER);remaining.setGravity(Gravity.CENTER);year.setGravity(Gravity.CENTER);
    progress=new ProgressBar(context,null,android.R.attr.progressBarStyleHorizontal);
    progress.setProgressTintList(android.content.res.ColorStateList.valueOf(Ui.ACCENT));
    progress.setProgressBackgroundTintList(android.content.res.ColorStateList.valueOf(0xff383838));
    LinearLayout.LayoutParams progressLayout=new LinearLayout.LayoutParams(-1,Ui.dp(context,2));
    progressLayout.topMargin=Ui.dp(context,14);
    addView(progress,1,progressLayout);
    setOnClickListener(v->{if(item!=null)open.accept(item);});
    setOnFocusChangeListener((v,f)->{
      focusAppearance(f);
      if(f&&item!=null) {
        focused.accept(item);
        post(()->requestRectangleOnScreen(new android.graphics.Rect(0,0,getWidth(),getHeight()),false));
      }
    });
  }

  void bind(Library.Title value) {
    item=value; setTag("title:"+value.id);
    setContentDescription(value.title+", "+value.year+(value.watched()?", Watched":""));
    title.setText(value.title);title.setVisibility(value.poster.isEmpty()?VISIBLE:GONE);
    watched.setVisibility(value.watched()?VISIBLE:GONE);
    Library.Asset resume=value.resume();
    subtitle.setText(resume!=null ? (resume.position==0?"Up next · "+resume.label():resume.status()) : value.watched()?L10n.t("Watched"):value.year);
    progress.setVisibility(resume!=null&&resume.duration>0?VISIBLE:INVISIBLE);
    progress.setProgress(resume!=null&&resume.duration>0?(int)(100*resume.position/resume.duration):0);
    updateCaption();
    art.load(poster,value.id,"poster",value.poster);
  }

  void showContinuation(boolean enabled) { continuing=enabled;updateCaption(); }
  private void updateCaption() {
    remaining.setVisibility(GONE);year.setVisibility(GONE);
    progress.setVisibility(continuing?VISIBLE:GONE);
    if(item==null)return;
    subtitle.setVisibility(VISIBLE);
    subtitle.setText((item.genres.isEmpty()?"":item.genres.get(0))+(item.genres.isEmpty()||item.year.isEmpty()?"":" · ")+item.year);
    if(!continuing)return;
    year.setText(item.year);year.setVisibility(item.year.isEmpty()?GONE:VISIBLE);
    Library.Asset resume=item.resume();if(resume==null)return;
    if(item.kind.equals("tv") && resume.hasSeason) {
      subtitle.setText(String.format(L10n.locale(),L10n.t("Season %d Ep %d"),resume.season,resume.episode));
      subtitle.setVisibility(VISIBLE);
    } else { subtitle.setVisibility(GONE); }
    if(resume.duration>0) {
      int minutes=(int)Math.ceil(Math.max(0,resume.duration-resume.position)/60);
      remaining.setText(minutes>=60 ? String.format(L10n.locale(),L10n.t("%dh %dm left"),minutes/60,minutes%60)
          : String.format(L10n.locale(),L10n.t("%dm left"),minutes));
      remaining.setVisibility(VISIBLE);
    }
  }

  void focusAppearance(boolean focused) {
    artwork.animate().cancel();
    android.graphics.drawable.GradientDrawable outline=Ui.shape(0,0,getContext());
    outline.setStroke(Ui.dp(getContext(),1),0xa6ffffff);
    artwork.setForeground(focused?outline:null);
    artwork.animate().scaleX(focused?1.045f:1).scaleY(focused?1.045f:1).setDuration(150).start();
    // Visual growth does not change layout or scroll geometry.
    title.setTextColor(focused?Color.WHITE:Ui.TEXT);
    subtitle.setTextColor(focused?0xffd2dedb:Ui.MUTED);
    artwork.setElevation(Ui.dp(getContext(),focused?6:0));
  }

  @Override protected void onMeasure(int widthSpec,int heightSpec) {
    int width=MeasureSpec.getSize(widthSpec);
    if(width>0)artwork.getLayoutParams().height=width*3/2;
    super.onMeasure(widthSpec,heightSpec);
  }
}
