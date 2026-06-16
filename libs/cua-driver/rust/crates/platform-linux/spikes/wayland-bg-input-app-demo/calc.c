/* A small but real GTK4 calculator. Also exports each button's on-screen
 * center to /tmp/calc_coords.txt so a driver can click them (stands in for the
 * coordinates an agent would read from an accessibility tree). */
#include <gtk/gtk.h>
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static GtkWidget *win, *display;
static GPtrArray *btns;
static double acc = 0; static char pending = 0; static gboolean fresh = TRUE;
static char cur[64] = "0";

static void refresh(void) {
	char *m = g_markup_printf_escaped("<span font='34' weight='bold'>%s</span>", cur);
	gtk_label_set_markup(GTK_LABEL(display), m); g_free(m);
}
static void digit(const char *d) {
	if (fresh) { g_strlcpy(cur, d, sizeof cur); fresh = FALSE; }
	else if (!strcmp(cur, "0")) g_strlcpy(cur, d, sizeof cur);
	else { size_t n = strlen(cur); if (n < sizeof cur - 1) { cur[n] = d[0]; cur[n+1] = 0; } }
	refresh();
}
static void setop(char op) { acc = atof(cur); pending = op; fresh = TRUE; }
static void equals(void) {
	double b = atof(cur), r = b;
	switch (pending) { case '+': r = acc+b; break; case '-': r = acc-b; break;
		case '*': r = acc*b; break; case '/': r = b ? acc/b : 0; break; }
	snprintf(cur, sizeof cur, "%g", r); pending = 0; fresh = TRUE; refresh();
}
static void on_btn(GtkButton *b, gpointer u) {
	const char *l = gtk_button_get_label(b);
	if (isdigit((unsigned char)l[0])) digit(l);
	else if (!strcmp(l, "C")) { g_strlcpy(cur, "0", sizeof cur); acc = 0; pending = 0; fresh = TRUE; refresh(); }
	else if (!strcmp(l, "=")) equals();
	else setop(l[0]);
}
static gboolean tick(gpointer u) { gtk_widget_queue_draw(win); return G_SOURCE_CONTINUE; }
static gboolean export_coords(gpointer u) {
	FILE *f = fopen("/tmp/calc_coords.txt", "w");
	for (guint i = 0; i < btns->len; i++) {
		GtkWidget *b = g_ptr_array_index(btns, i);
		graphene_rect_t r;
		if (gtk_widget_compute_bounds(b, win, &r))
			fprintf(f, "%s %.0f %.0f\n", gtk_button_get_label(GTK_BUTTON(b)),
				r.origin.x + r.size.width/2, r.origin.y + r.size.height/2);
	}
	fclose(f);
	printf("calc: exported button coords\n"); fflush(stdout);
	return G_SOURCE_REMOVE;
}
int main(int argc, char **argv) {
	gtk_init();
	win = gtk_window_new();
	int ww = getenv("CUA_WW") ? atoi(getenv("CUA_WW")) : 360;
	int wh = getenv("CUA_WH") ? atoi(getenv("CUA_WH")) : 520;
	gtk_window_set_default_size(GTK_WINDOW(win), ww, wh);
	gtk_window_set_title(GTK_WINDOW(win), "calc");
	GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
	gtk_widget_set_margin_top(box, 8); gtk_widget_set_margin_bottom(box, 8);
	gtk_widget_set_margin_start(box, 8); gtk_widget_set_margin_end(box, 8);
	display = gtk_label_new(NULL); gtk_label_set_xalign(GTK_LABEL(display), 1.0);
	gtk_widget_set_size_request(display, -1, 70); refresh();
	gtk_box_append(GTK_BOX(box), display);
	GtkWidget *grid = gtk_grid_new();
	gtk_grid_set_row_spacing(GTK_GRID(grid), 6); gtk_grid_set_column_spacing(GTK_GRID(grid), 6);
	gtk_widget_set_vexpand(grid, TRUE); gtk_box_append(GTK_BOX(box), grid);
	const char *lay[4][4] = {{"7","8","9","/"},{"4","5","6","*"},{"1","2","3","-"},{"C","0","=","+"}};
	btns = g_ptr_array_new();
	for (int r = 0; r < 4; r++) for (int c = 0; c < 4; c++) {
		GtkWidget *b = gtk_button_new_with_label(lay[r][c]);
		gtk_widget_set_hexpand(b, TRUE); gtk_widget_set_vexpand(b, TRUE);
		g_signal_connect(b, "clicked", G_CALLBACK(on_btn), NULL);
		gtk_grid_attach(GTK_GRID(grid), b, c, r, 1, 1);
		g_ptr_array_add(btns, b);
	}
	gtk_window_set_child(GTK_WINDOW(win), box);
	gtk_window_present(GTK_WINDOW(win));
	g_timeout_add(700, export_coords, NULL);
	g_timeout_add(80, tick, NULL); /* keep the headless output producing frames */
	while (TRUE) g_main_context_iteration(NULL, TRUE);
	return 0;
}
