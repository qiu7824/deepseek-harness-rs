#include "my_application.h"

#include <flutter_linux/flutter_linux.h>
#ifdef GDK_WINDOWING_X11
#include <gdk/gdkx.h>
#endif

#include <cstring>
#include <cstdint>

#include "flutter/generated_plugin_registrant.h"

struct _MyApplication {
  GtkApplication parent_instance;
  char** dart_entrypoint_arguments;
  FlMethodChannel* clipboard_channel;
};

G_DEFINE_TYPE(MyApplication, my_application, GTK_TYPE_APPLICATION)

// Replies to one dsh/clipboard read with copied files or a PNG image.
static void clipboard_respond(FlMethodCall* method_call, FlValue* files,
                              GdkPixbuf* pixbuf) {
  g_autoptr(FlValue) payload = fl_value_new_map();
  fl_value_set_string_take(payload, "files", files);
  if (pixbuf != nullptr) {
    const int width = gdk_pixbuf_get_width(pixbuf);
    const int height = gdk_pixbuf_get_height(pixbuf);
    if (width <= 0 || height <= 0 ||
        static_cast<uint64_t>(width) * height > 32000000) {
      fl_method_call_respond_error(method_call, "clipboard-too-large",
                                  "Clipboard image dimensions exceed the limit", nullptr, nullptr);
      return;
    }
    gchar* buffer = nullptr;
    gsize size = 0;
    if (gdk_pixbuf_save_to_buffer(pixbuf, &buffer, &size, "png", nullptr,
                                  nullptr)) {
      if (size > 16 * 1024 * 1024) {
        g_free(buffer);
        fl_method_call_respond_error(method_call, "clipboard-too-large",
                                    "Clipboard PNG exceeds 16 MiB", nullptr, nullptr);
        return;
      }
      fl_value_set_string_take(
          payload, "png",
          fl_value_new_uint8_list(reinterpret_cast<const uint8_t*>(buffer),
                                  size));
      g_free(buffer);
    }
  }
  g_autoptr(FlMethodResponse) response =
      FL_METHOD_RESPONSE(fl_method_success_response_new(payload));
  fl_method_call_respond(method_call, response, nullptr);
}

static void clipboard_image_cb(GtkClipboard* clipboard, GdkPixbuf* pixbuf,
                               gpointer data) {
  g_autoptr(FlMethodCall) method_call = FL_METHOD_CALL(data);
  clipboard_respond(method_call, fl_value_new_list(), pixbuf);
}

// Files take precedence; image content is requested only without files.
static void clipboard_uris_cb(GtkClipboard* clipboard, gchar** uris,
                              gpointer data) {
  FlMethodCall* method_call = FL_METHOD_CALL(data);
  FlValue* files = fl_value_new_list();
  for (gchar** uri = uris; uri != nullptr && *uri != nullptr; uri++) {
    g_autofree gchar* path = g_filename_from_uri(*uri, nullptr, nullptr);
    if (path != nullptr) fl_value_append_take(files, fl_value_new_string(path));
    if (fl_value_get_length(files) > 8) {
      fl_value_unref(files);
      fl_method_call_respond_error(method_call, "clipboard-too-many-files",
                                  "Clipboard contains more than eight files", nullptr, nullptr);
      g_object_unref(method_call);
      return;
    }
  }
  if (fl_value_get_length(files) > 0) {
    clipboard_respond(method_call, files, nullptr);
    g_object_unref(method_call);
    return;
  }
  fl_value_unref(files);
  gtk_clipboard_request_image(clipboard, clipboard_image_cb, method_call);
}

static void clipboard_method_cb(FlMethodChannel* channel,
                                FlMethodCall* method_call,
                                gpointer user_data) {
  if (strcmp(fl_method_call_get_name(method_call), "read") != 0) {
    fl_method_call_respond_not_implemented(method_call, nullptr);
    return;
  }
  GtkClipboard* clipboard = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
  gtk_clipboard_request_uris(clipboard, clipboard_uris_cb,
                             g_object_ref(method_call));
}

// Called when first Flutter frame received.
static void first_frame_cb(MyApplication* self, FlView* view) {
  gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));
}

// Implements GApplication::activate.
static void my_application_activate(GApplication* application) {
  MyApplication* self = MY_APPLICATION(application);
  GtkWindow* window =
      GTK_WINDOW(gtk_application_window_new(GTK_APPLICATION(application)));

  // Portable bundles also expose their hicolor theme without registration.
  g_autofree gchar* executable = g_file_read_link("/proc/self/exe", nullptr);
  if (executable != nullptr) {
    g_autofree gchar* directory = g_path_get_dirname(executable);
    g_autofree gchar* icons = g_build_filename(directory, "share", "icons", nullptr);
    gtk_icon_theme_append_search_path(gtk_icon_theme_get_default(), icons);
  }
  gtk_window_set_icon_name(window, APPLICATION_ID);

  // Use a header bar when running in GNOME as this is the common style used
  // by applications and is the setup most users will be using (e.g. Ubuntu
  // desktop).
  // If running on X and not using GNOME then just use a traditional title bar
  // in case the window manager does more exotic layout, e.g. tiling.
  // If running on Wayland assume the header bar will work (may need changing
  // if future cases occur).
  gboolean use_header_bar = TRUE;
#ifdef GDK_WINDOWING_X11
  GdkScreen* screen = gtk_window_get_screen(window);
  if (GDK_IS_X11_SCREEN(screen)) {
    const gchar* wm_name = gdk_x11_screen_get_window_manager_name(screen);
    if (g_strcmp0(wm_name, "GNOME Shell") != 0) {
      use_header_bar = FALSE;
    }
  }
#endif
  if (use_header_bar) {
    GtkHeaderBar* header_bar = GTK_HEADER_BAR(gtk_header_bar_new());
    gtk_widget_show(GTK_WIDGET(header_bar));
    gtk_header_bar_set_title(header_bar, "DeepSeek Harness");
    gtk_header_bar_set_show_close_button(header_bar, TRUE);
    gtk_window_set_titlebar(window, GTK_WIDGET(header_bar));
  } else {
    gtk_window_set_title(window, "DeepSeek Harness");
  }

  gtk_window_set_default_size(window, 1280, 720);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(
      project, self->dart_entrypoint_arguments);

  FlView* view = fl_view_new(project);
  GdkRGBA background_color;
  // Background defaults to black, override it here if necessary, e.g. #00000000
  // for transparent.
  gdk_rgba_parse(&background_color, "#000000");
  fl_view_set_background_color(view, &background_color);
  gtk_widget_show(GTK_WIDGET(view));
  gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(view));

  // Show the window when Flutter renders.
  // Requires the view to be realized so we can start rendering.
  g_signal_connect_swapped(view, "first-frame", G_CALLBACK(first_frame_cb),
                           self);
  gtk_widget_realize(GTK_WIDGET(view));

  fl_register_plugins(FL_PLUGIN_REGISTRY(view));

  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  self->clipboard_channel = fl_method_channel_new(
      fl_engine_get_binary_messenger(fl_view_get_engine(view)),
      "dsh/clipboard", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(
      self->clipboard_channel, clipboard_method_cb, self, nullptr);

  gtk_widget_grab_focus(GTK_WIDGET(view));
}

// Implements GApplication::local_command_line.
static gboolean my_application_local_command_line(GApplication* application,
                                                  gchar*** arguments,
                                                  int* exit_status) {
  MyApplication* self = MY_APPLICATION(application);
  // Strip out the first argument as it is the binary name.
  self->dart_entrypoint_arguments = g_strdupv(*arguments + 1);

  g_autoptr(GError) error = nullptr;
  if (!g_application_register(application, nullptr, &error)) {
    g_warning("Failed to register: %s", error->message);
    *exit_status = 1;
    return TRUE;
  }

  g_application_activate(application);
  *exit_status = 0;

  return TRUE;
}

// Implements GApplication::startup.
static void my_application_startup(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application startup.

  G_APPLICATION_CLASS(my_application_parent_class)->startup(application);
}

// Implements GApplication::shutdown.
static void my_application_shutdown(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application shutdown.

  G_APPLICATION_CLASS(my_application_parent_class)->shutdown(application);
}

// Implements GObject::dispose.
static void my_application_dispose(GObject* object) {
  MyApplication* self = MY_APPLICATION(object);
  g_clear_pointer(&self->dart_entrypoint_arguments, g_strfreev);
  g_clear_object(&self->clipboard_channel);
  G_OBJECT_CLASS(my_application_parent_class)->dispose(object);
}

static void my_application_class_init(MyApplicationClass* klass) {
  G_APPLICATION_CLASS(klass)->activate = my_application_activate;
  G_APPLICATION_CLASS(klass)->local_command_line =
      my_application_local_command_line;
  G_APPLICATION_CLASS(klass)->startup = my_application_startup;
  G_APPLICATION_CLASS(klass)->shutdown = my_application_shutdown;
  G_OBJECT_CLASS(klass)->dispose = my_application_dispose;
}

static void my_application_init(MyApplication* self) {}

MyApplication* my_application_new() {
  // Set the program name to the application ID, which helps various systems
  // like GTK and desktop environments map this running application to its
  // corresponding .desktop file. This ensures better integration by allowing
  // the application to be recognized beyond its binary name.
  g_set_prgname(APPLICATION_ID);

  return MY_APPLICATION(g_object_new(my_application_get_type(),
                                     "application-id", APPLICATION_ID, "flags",
                                     G_APPLICATION_NON_UNIQUE, nullptr));
}
