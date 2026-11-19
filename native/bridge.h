#pragma once
#include <stdint.h>
#ifdef HDR_ABC_BUILD
#define ABC_API extern "C" __declspec(dllexport)
#else
#define ABC_API extern "C" __declspec(dllimport)
#endif

// Borrowed buffers stay valid until abc_close. Never free them in Rust.
// All matrices are column-major for column vectors (transpose of Imath).
struct AbcNode {
    const char* path;
    int32_t parent;
    uint32_t kind; // 0 group, 1 transform, 2 mesh, 3 camera, 4 unsupported
    double world[16];
    const float* positions;
    uint64_t position_count;
    const int32_t* indices;
    uint64_t index_count;
    const int32_t* face_counts;
    uint64_t face_count;
    const float* corner_normals;
    uint64_t normal_count;
    double camera_core[16]; // Alembic CameraSample core values, original units
    double film_back[9]; // 3x3 column-major
    uint32_t visible;
};
ABC_API uint32_t abc_abi_version();
ABC_API void* abc_open(const char* utf8_path, double seconds, char* error, uint64_t capacity);
ABC_API uint64_t abc_node_count(const void* scene);
ABC_API int32_t abc_get_node(const void* scene, uint64_t index, AbcNode* out);
ABC_API void abc_close(void* scene);
