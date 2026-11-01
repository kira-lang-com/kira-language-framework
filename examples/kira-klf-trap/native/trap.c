extern void klfrt_trap(int);

long long klf_test_trap(void) {
    klfrt_trap(7);
    return 0;
}
