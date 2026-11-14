#include <QApplication>
#include <QPushButton>
#include <QMouseEvent>
#include <cstdio>

class Btn : public QPushButton {
public:
	Btn(const QString &t) : QPushButton(t) { setMouseTracking(true); }
protected:
	void enterEvent(QEnterEvent *e) override {
		printf("QT6: ENTER at %d,%d\n", (int)e->position().x(), (int)e->position().y());
		fflush(stdout);
		QPushButton::enterEvent(e);
	}
	void mousePressEvent(QMouseEvent *e) override {
		printf("QT6: PRESSED at %d,%d\n", (int)e->position().x(), (int)e->position().y());
		fflush(stdout);
		QPushButton::mousePressEvent(e);
	}
};

int main(int argc, char **argv) {
	QApplication a(argc, argv);
	Btn b("hit me");
	QObject::connect(&b, &QPushButton::clicked, [] {
		printf("QT6: button CLICKED (activated)\n");
		fflush(stdout);
	});
	b.resize(200, 150);
	b.setWindowTitle("cua-spike-qt6");
	b.show();
	printf("QT6: window shown\n");
	fflush(stdout);
	return a.exec();
}
