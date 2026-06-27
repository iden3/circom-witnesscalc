#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../include/graph_witness.h"

void read_json_file(const char* file_path, char** data) {
    FILE *file = fopen(file_path, "r");
    if (!file) {
        perror("Failed to open inputs JSON file");
        exit(EXIT_FAILURE);
    }

    fseek(file, 0, SEEK_END);
    long file_length = ftell(file);
    if (file_length < 0) {
        perror("Failed to determine inputs JSON file size");
        fclose(file);
        exit(EXIT_FAILURE);
    }
    size_t length = (size_t)file_length;
    fseek(file, 0, SEEK_SET);

    *data = malloc(length + 1);
    if (!*data) {
        perror("Failed to allocate memory for JSON inputs");
        fclose(file);
        exit(EXIT_FAILURE);
    }

    size_t sz = fread(*data, 1, length, file);
	fclose(file);

	if (sz != length) {
	  fprintf(stderr, "Failed to read JSON inputs");
	  exit(EXIT_FAILURE);
	}
    (*data)[length] = '\0';

	size_t sz2 = strlen(*data);
	if (sz2 != length) {
	  fprintf(stderr, "Something is wrong with inputs JSON data. Is it a correct JSON?");
	  exit(EXIT_FAILURE);
	}
}

void read_binary_file(const char* file_path, void** binary_data, size_t* binary_length) {
    FILE *file = fopen(file_path, "rb");
    if (!file) {
        perror("Failed to open binary file");
        exit(EXIT_FAILURE);
    }

    fseek(file, 0, SEEK_END);
    long file_length = ftell(file);
    if (file_length < 0) {
        perror("Failed to determine binary file size");
        fclose(file);
        exit(EXIT_FAILURE);
    }
    size_t length = (size_t)file_length;
    fseek(file, 0, SEEK_SET);

    void *data = malloc(length);
    if (!data) {
        perror("Failed to allocate memory for binary data");
        fclose(file);
        exit(EXIT_FAILURE);
    }

    size_t sz = fread(data, 1, length, file);
    fclose(file);

	if (sz != length) {
	  fprintf(stderr, "Failed to read a file %s", file_path);
	  exit(EXIT_FAILURE);
	}

    *binary_data = data;
    *binary_length = length;
}


void save_binary_file(const char* file_path, const void* data, size_t length) {
    FILE *file = fopen(file_path, "wb");
    if (!file) {
        perror("Failed to open output file");
        exit(EXIT_FAILURE);
    }

    size_t sz = fwrite(data, 1, length, file);
    fclose(file);
	
	if (sz != length) {
	  fprintf(stderr, "Failed to write a file %s", file_path);
	  exit(EXIT_FAILURE);
	}
}


int
main(int argc, char *argv[]) {
  if (argc != 4) {
	fprintf(stderr, "Usage: %s <inputs> <circuit_graph> <witness>\n", argv[0]);
	return EXIT_FAILURE;
  }

  const char* inputs_json_path = argv[1];
  const char* circuit_graph_path = argv[2];
  const char* witness_path = argv[3];

  char* inputs_json_data;
  read_json_file(inputs_json_path, &inputs_json_data);

  void* graph_data;
  size_t graph_length;
  read_binary_file(circuit_graph_path, &graph_data, &graph_length);

  void *wtns_data = NULL;
  size_t wtns_len = 0;
 
  gw_status_t status = { .code = OK, .error_msg = NULL };
  int r = gw_calc_witness(inputs_json_data, graph_data, graph_length, &wtns_data, &wtns_len, &status);
  if (r != 0) {
	fprintf(stderr, "Error code: %i\n", status.code);
	if (status.error_msg != NULL) {
	  printf("Error msg: %s\n", status.error_msg);
	}
	gw_free_status(&status);
	free(inputs_json_data);
	free(graph_data);
	return 1;
  }
  gw_free_status(&status);

  free(inputs_json_data);
  free(graph_data);

  save_binary_file(witness_path, wtns_data, wtns_len);
  gw_free_wtns_data(wtns_data);
}
