package voice.live;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileNotFoundException;
import java.util.ArrayList;

public final class PhotoProvider extends ContentProvider {
    private static File dir(Context context) { return new File(context.getCacheDir(), "photos"); }
    private static File file(Context context, Uri uri) {
        String name = uri.getLastPathSegment();
        return "voice.live.photo".equals(uri.getAuthority()) && name != null && name.matches("photo-\\d+\\.jpg")
            ? new File(dir(context), name) : null;
    }
    static boolean taken(Context context, Uri photo) { return photo != null && file(context, photo).length() > 0; }
    static void clear(Context context) {
        File[] photos = dir(context).listFiles();
        if (photos != null) for (File photo : photos) photo.delete();
    }
    static Intent picker(Context context, String[] accept, Uri photo) {
        ArrayList<String> types = new ArrayList<>();
        for (String type : accept) if (type.contains("/")) types.add(type.trim());
        Intent pick = new Intent(Intent.ACTION_GET_CONTENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*");
        if (!types.isEmpty()) pick.putExtra(Intent.EXTRA_MIME_TYPES, types.toArray(new String[0]));
        Intent chooser = Intent.createChooser(pick, null);
        if (types.isEmpty() || types.stream().anyMatch(type -> type.startsWith("image/"))) {
            dir(context).mkdirs();
            chooser.putExtra(Intent.EXTRA_INITIAL_INTENTS, new Intent[]{new Intent(MediaStore.ACTION_IMAGE_CAPTURE)
                .putExtra(MediaStore.EXTRA_OUTPUT, photo)
                .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION | Intent.FLAG_GRANT_READ_URI_PERMISSION)});
        }
        return chooser;
    }
    static Uri next() { return Uri.parse("content://voice.live.photo/photo-" + System.nanoTime() + ".jpg"); }
    public boolean onCreate() { return true; }
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        File photo = file(getContext(), uri);
        if (photo == null) throw new FileNotFoundException(uri.toString());
        return ParcelFileDescriptor.open(photo, ParcelFileDescriptor.parseMode(mode));
    }
    public Cursor query(Uri uri, String[] projection, String selection, String[] args, String order) {
        MatrixCursor cursor = new MatrixCursor(new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE});
        File photo = file(getContext(), uri);
        if (photo != null) cursor.addRow(new Object[]{"photo.jpg", photo.length()});
        return cursor;
    }
    public String getType(Uri uri) { return file(getContext(), uri) != null ? "image/jpeg" : null; }
    public Uri insert(Uri uri, ContentValues values) { return null; }
    public int delete(Uri uri, String selection, String[] args) { return 0; }
    public int update(Uri uri, ContentValues values, String selection, String[] args) { return 0; }
}
