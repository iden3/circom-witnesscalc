#include <stddef.h>
#include <stdlib.h>

#ifndef RUST_GRAPH_WITNESS_H
#define RUST_GRAPH_WITNESS_H

typedef enum {
  OK = 0,
  ERROR = 1
} GW_ERROR_CODE;

// Callers own status.error_msg. Initialize it to NULL before first use and call
// gw_free_status before reusing or discarding a status. ERROR may have a NULL
// error_msg if allocating the message fails.
typedef struct {
  GW_ERROR_CODE code;
  char *error_msg;
} gw_status_t;

// On success, gw_calc_witness writes heap-allocated witness bytes to wtns_data
// and their length to wtns_len. Release wtns_data with gw_free_wtns_data.
// If either output pointer is NULL, gw_calc_witness returns an error without
// writing witness outputs. After both output pointers have been validated,
// every error resets *wtns_data to NULL and *wtns_len to 0.
// status.error_msg is owned separately and must be released with gw_free_status.
int
gw_calc_witness(const char *inputs,
				const void *graph_data, const size_t graph_data_len,
			    void **wtns_data, size_t *wtns_len,
				gw_status_t *status);

void
gw_free_wtns_data(void *wtns_data);

static inline void
gw_free_status(gw_status_t *status) {
  if (status != NULL && status->error_msg != NULL) {
	free(status->error_msg);
	status->error_msg = NULL;
  }
}

#endif // RUST_GRAPH_WITNESS_H
