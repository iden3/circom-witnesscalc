#include <stddef.h>
#include <stdlib.h>

#ifndef RUST_GRAPH_WITNESS_H
#define RUST_GRAPH_WITNESS_H

typedef enum {
  OK = 0,
  ERROR = 1
} GW_ERROR_CODE;

// Callers own error_msg. Initialize it to NULL before first use and call
// gw_free_status before reusing or discarding a status. ERROR may have a NULL
// error_msg if allocating the message fails.
typedef struct {
  GW_ERROR_CODE code;
  char *error_msg;
} gw_status_t;

int
gw_calc_witness(const char *inputs,
				const void *graph_data, const size_t graph_data_len,
			    void **wtns_data, size_t *wtns_len,
				gw_status_t *status);

static inline void
gw_free_status(gw_status_t *status) {
  if (status != NULL && status->error_msg != NULL) {
	free(status->error_msg);
	status->error_msg = NULL;
  }
}

#endif // RUST_GRAPH_WITNESS_H
