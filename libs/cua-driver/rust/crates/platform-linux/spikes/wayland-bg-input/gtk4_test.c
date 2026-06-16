#include <gtk/gtk.h>

static void on_clicked(GtkButton *b, gpointer u) {
	g_print("GTK4: button CLICKED (activated)\n");
}
static void on_pressed(GtkGestureClick *g, int n, double x, double y, gpointer u) {
	g_print("GTK4: gesture PRESSED at %.0f,%.0f\n", x, y);
}
static void on_enter(GtkEventControllerMotion *m, double x, double y, gpointer u) {
	g_print("GTK4: pointer ENTER at %.0f,%.0f\n", x, y);
}
static void on_motion(GtkEventControllerMotion *m, double x, double y, gpointer u) {
	g_print("GTK4: pointer MOTION at %.0f,%.0f\n", x, y);
}

int main(int argc, char **argv) {
	gtk_init();
	GtkWidget *win = gtk_window_new();
	gtk_window_set_default_size(GTK_WINDOW(win), 200, 150);
	gtk_window_set_title(GTK_WINDOW(win), "cua-spike-gtk4");

	GtkWidget *btn = gtk_button_new_with_label("hit me");
	g_signal_connect(btn, "clicked", G_CALLBACK(on_clicked), NULL);

	GtkGesture *click = gtk_gesture_click_new();
	g_signal_connect(click, "pressed", G_CALLBACK(on_pressed), NULL);
	gtk_widget_add_controller(btn, GTK_EVENT_CONTROLLER(click));

	GtkEventController *mot = gtk_event_controller_motion_new();
	g_signal_connect(mot, "enter", G_CALLBACK(on_enter), NULL);
	g_signal_connect(mot, "motion", G_CALLBACK(on_motion), NULL);
	gtk_widget_add_controller(btn, mot);

	gtk_window_set_child(GTK_WINDOW(win), btn);
	gtk_window_present(GTK_WINDOW(win));
	g_print("GTK4: window presented\n");

	while (TRUE) {
		g_main_context_iteration(NULL, TRUE);
	}
	return 0;
}
