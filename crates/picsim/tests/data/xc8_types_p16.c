#include <xc.h>
#pragma config WDTE = OFF, LVP = ON
enum color { RED, GREEN = 5, BLUE };
struct point { unsigned char x; int y; };
struct shape { struct point origin; unsigned char tag[3]; enum color c; };
union word { unsigned int w; unsigned char b[2]; };
typedef struct point point_t;
struct point pt;
point_t pts[2];
struct node { unsigned char v; struct node *next; };
struct node n1, n2;
struct flags { unsigned a:3; unsigned b:2; } fl;
struct shape sh;
union word uw;
enum color col;
unsigned char data[4];
unsigned char *p;
struct point *pp;
void main(void)
{
    pt.x = 7;
    pt.y = -300;
    pts[1].x = 9;
    sh.origin.x = 3;
    sh.tag[1] = 'k';
    sh.c = BLUE;
    uw.w = 0x1234;
    col = GREEN;
    data[2] = 42;
    p = &data[2];
    pp = &pt;
    n1.v = 1; n1.next = &n2; n2.v = 2;
    fl.a = 5; fl.b = 2;
    while (1) {
        pt.x++;
    }
}
