package com.torrbaby.tv;

import android.graphics.Bitmap;
import android.graphics.Color;
import com.google.zxing.BarcodeFormat;
import com.google.zxing.EncodeHintType;
import com.google.zxing.common.BitMatrix;
import com.google.zxing.qrcode.QRCodeWriter;
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel;
import java.util.EnumMap;
import java.util.Map;

/** A QR code as a bitmap: black modules on white with a quiet zone, whole pixels per module. */
final class QrCode {
  static Bitmap bitmap(String text, int pixels) throws Exception {
    Map<EncodeHintType, Object> hints = new EnumMap<>(EncodeHintType.class);
    hints.put(EncodeHintType.ERROR_CORRECTION, ErrorCorrectionLevel.M);
    hints.put(EncodeHintType.CHARACTER_SET, "UTF-8");
    hints.put(EncodeHintType.MARGIN, 4);
    BitMatrix modules = new QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 0, 0, hints);
    int size = modules.getWidth(), scale = Math.max(1, pixels / size), side = size * scale;
    int[] colors = new int[side * side];
    for (int y = 0; y < side; y++)
      for (int x = 0; x < side; x++)
        colors[y * side + x] = modules.get(x / scale, y / scale) ? Color.BLACK : Color.WHITE;
    return Bitmap.createBitmap(colors, side, side, Bitmap.Config.ARGB_8888);
  }
}
