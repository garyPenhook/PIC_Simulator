// Constants in program memory: pointers to string literals and tables, and a table of string pointers.
#include <xc.h>
const char *msg;
const unsigned char *tp;
char buf[8] = "abc";
const char *rp;                      // points into data memory
const unsigned char tab[3] = {7, 8, 9};
const char *const names[2] = {"ab", "cd"};
char second;

void main(void) {
    msg = "world";
    tp = tab;
    rp = buf;
    second = names[1][0];
    while (1) {
    }
}
