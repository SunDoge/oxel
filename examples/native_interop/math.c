#include <stdint.h>
#include <string.h>

int32_t oxel_add(int32_t left, int32_t right) {
    return left + right;
}

double oxel_scale(double value, double factor) {
    return value * factor;
}

typedef struct {
    int16_t channel;
    double value;
} Reading;

typedef struct {
    Reading reading;
    int32_t count;
} Batch;

Reading oxel_adjust(Reading sample, int16_t offset, double factor) {
    return (Reading){ sample.channel + offset, sample.value * factor };
}

Batch oxel_combine(Batch left, Batch right) {
    return (Batch){
        { left.reading.channel + right.reading.channel,
          left.reading.value + right.reading.value },
        left.count + right.count
    };
}

void oxel_record(int32_t delta, int32_t *total, Reading *reading, Batch *result) {
    *total += delta;
    reading->value += delta;
    *result = (Batch){ *reading, *total };
}

int64_t oxel_utf8_length(const char *text) {
    return text ? (int64_t)strlen(text) : -1;
}
