package com.torrbaby.tv;

import android.app.*;
import android.content.*;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.text.*;
import android.view.*;
import android.widget.*;
import org.json.*;

final class ProfileUi {
  interface Save { void apply(JSONObject values, AlertDialog dialog); }
  static int color(String hex,int fallback) { try{return Color.parseColor(hex);}catch(Exception e){return fallback;} }
  static String initial(String name) { return name.isEmpty()?"?":new String(Character.toChars(name.codePointAt(0))).toUpperCase(java.util.Locale.ROOT); }
  static TextView avatar(Activity a,JSONObject person,int size) {
    TextView view=Ui.title(a, "", size/2);
    view.setGravity(Gravity.CENTER);
    updateAvatar(a,view,person,size);
    return view;
  }
  static void updateAvatar(Activity a,TextView view,JSONObject person,int size) {
    String symbol=person.optString("avatar_symbol");
    view.setText(symbol.isEmpty()?initial(person.optString("name")):symbol);
    view.setTextColor(color(person.optString("text_color"),Ui.ACCENT));
    GradientDrawable background=new GradientDrawable();background.setShape(GradientDrawable.OVAL);
    background.setColor(color(person.optString("background_color"),0xff243a38));
    view.setBackground(background);
    view.setLayoutParams(new LinearLayout.LayoutParams(Ui.dp(a,size),Ui.dp(a,size)));
    view.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
  }
  static JSONObject person(Api api) {
    if(api.profileAppearance!=null && !api.profileAppearance.optString("name").isEmpty())return api.profileAppearance;
    JSONObject value=new JSONObject();try{value.put("id",api.profile).put("name",api.profileName);}catch(Exception ignored){}return value;
  }
  static LinearLayout badge(Activity a,Api api,Runnable action) {
    LinearLayout button=Ui.row(a);button.setGravity(Gravity.CENTER_VERTICAL);Ui.focus(button);Ui.pad(button,10,4);
    button.setTag("profile-badge");
    JSONObject person=person(api);button.addView(avatar(a,person,36));Ui.gap(button,10);
    Ui.add(button,Ui.text(a,person.optString("name"),15,Ui.TEXT),-2,-2);
    button.setContentDescription(L10n.t("Switch user")+": "+person.optString("name"));button.setOnClickListener(v->action.run());
    return button;
  }
  static void refreshBadges(Activity a, View view, Api api) {
    if ("profile-badge".equals(view.getTag()) && view instanceof LinearLayout) {
      LinearLayout badge = (LinearLayout)view;
      JSONObject person = person(api);
      updateAvatar(a, (TextView)badge.getChildAt(0), person, 36);
      ((TextView)badge.getChildAt(2)).setText(person.optString("name"));
      badge.setContentDescription(L10n.t("Switch user") + ": " + person.optString("name"));
      return;
    }
    if (view instanceof ViewGroup) {
      ViewGroup group = (ViewGroup)view;
      for (int i = 0; i < group.getChildCount(); i++) refreshBadges(a, group.getChildAt(i), api);
    }
  }
  static LinearLayout header(Activity a,Api api,Runnable action) {
    LinearLayout row=Ui.row(a);row.setGravity(Gravity.CENTER_VERTICAL);Ui.pad(row,16,4);
    if(api!=null&&!api.profile.isEmpty())row.addView(badge(a,api,action));
    row.addView(new View(a),new LinearLayout.LayoutParams(0,1,1));row.addView(L10n.picker(a));return row;
  }
  static LinearLayout header(Activity a,Api api) {
    return header(a,api,()->{
      a.startActivity(new Intent(a,MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP|Intent.FLAG_ACTIVITY_SINGLE_TOP).putExtra("switch_profile",true));a.finish();
    });
  }
  static void editor(Activity a,JSONObject person,Save save) {
    boolean creating=person==null;JSONObject prior=creating?new JSONObject():person;
    LinearLayout form=Ui.column(a);Ui.pad(form,24,16);form.setBackgroundColor(Ui.BG);
    TextView preview=avatar(a,prior,80);form.addView(preview);Ui.gap(form,14);
    EditText name=field(a,form,"First name",prior.optString("name"));
    EditText symbol=field(a,form,"Initial or emoji",prior.optString("avatar_symbol"));
    EditText foreground=field(a,form,"Text color",prior.optString("text_color","#70E8C2"));
    palette(a,form,foreground);
    EditText background=field(a,form,"Background color",prior.optString("background_color","#243A38"));
    palette(a,form,background);
    TextWatcher change=new TextWatcher(){public void beforeTextChanged(CharSequence s,int x,int c,int f){}public void onTextChanged(CharSequence s,int start,int before,int count){
      JSONObject values=values(name,symbol,foreground,background);updateAvatar(a,preview,values,80);
    }public void afterTextChanged(Editable e){}};
    for(EditText input:new EditText[]{name,symbol,foreground,background})input.addTextChangedListener(change);
    ScrollView scroll=new ScrollView(a);scroll.addView(form);
    AlertDialog dialog=new AlertDialog.Builder(a).setTitle(L10n.t(creating?"Create user":"Edit profile")).setView(scroll)
      .setNegativeButton(L10n.t("Cancel"),null).setPositiveButton(L10n.t("Save profile"),null).create();
    dialog.setOnShowListener(ignored->dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v->{
      if(name.getText().toString().trim().isEmpty()){name.setError(L10n.t("Enter a name"));return;}
      for(EditText input:new EditText[]{foreground,background})if(!input.getText().toString().matches("#[0-9a-fA-F]{6}")){input.setError("#RRGGBB");return;}
      save.apply(values(name,symbol,foreground,background),dialog);
    }));dialog.show();
  }
  private static JSONObject values(EditText name,EditText symbol,EditText foreground,EditText background){
    JSONObject value=new JSONObject();try{value.put("name",name.getText().toString().trim()).put("avatar_symbol",symbol.getText().toString()).put("text_color",foreground.getText().toString()).put("background_color",background.getText().toString());}catch(Exception ignored){}return value;
  }
  private static EditText field(Activity a,LinearLayout form,String label,String value){
    Ui.add(form,Ui.text(a,L10n.t(label),14,Ui.MUTED),-1,-2);
    EditText field=new EditText(a);field.setText(value);field.setSingleLine(true);field.setTextColor(Ui.TEXT);field.setContentDescription(L10n.t(label));
    Ui.add(form,field,-1,48);Ui.gap(form,10);return field;
  }
  private static void palette(Activity a,LinearLayout form,EditText field){
    HorizontalScrollView scroll=new HorizontalScrollView(a);LinearLayout row=Ui.row(a);
    for(String hex:new String[]{"#FFFFFF","#70E8C2","#F5C974","#F18FA8","#A89CF5","#7DC6FF","#243A38","#18202B","#4C2452","#6A3434"}){
      Button button=Ui.button(a,"●",()->field.setText(hex));button.setTextColor(color(hex,Ui.TEXT));button.setTextSize(26);button.setContentDescription(hex);Ui.add(row,button,48,44);Ui.gap(row,6);
    }scroll.addView(row);Ui.add(form,scroll,-1,48);Ui.gap(form,12);
  }
}
