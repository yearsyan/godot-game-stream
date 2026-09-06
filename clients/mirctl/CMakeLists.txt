cmake_minimum_required(VERSION 3.15)

project(mirctl VERSION 0.1.0 LANGUAGES C)

# ---------------------------------------------------------------- Language standard
set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)

# ---------------------------------------------------------------- Build type
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()

# ---------------------------------------------------------------- Conan toolchain
if(NOT DEFINED CMAKE_TOOLCHAIN_FILE)
  message(FATAL_ERROR
    "CMAKE_TOOLCHAIN_FILE is not set; first run: conan install . --build=missing")
endif()

find_package(FFmpeg CONFIG REQUIRED)
find_package(SDL3 CONFIG REQUIRED)

# ---------------------------------------------------------------- Core library (pure C)
add_library(mirctl_core STATIC
  src/control.c
  src/core.c
  src/decoder.c
  src/renderer.c
  src/transport.c
)
target_include_directories(mirctl_core PUBLIC src)
target_link_libraries(mirctl_core PUBLIC
  ffmpeg::ffmpeg
  SDL3::SDL3
)

if(WIN32)
  target_link_libraries(mirctl_core PRIVATE ws2_32 d3d11 dxgi dxva2 ole32)
elseif(UNIX)
  target_link_libraries(mirctl_core PRIVATE m)
endif()

# ---------------------------------------------------------------- Executable
add_executable(mirctl
  src/main.c
)
target_link_libraries(mirctl PRIVATE
  mirctl_core
)

# ---------------------------------------------------------------- Compiler warnings
if(CMAKE_C_COMPILER_ID MATCHES "GNU|Clang")
  target_compile_options(mirctl_core PRIVATE -Wall -Wextra -Wpedantic)
  target_compile_options(mirctl PRIVATE -Wall -Wextra -Wpedantic)
elseif(MSVC)
  # VS 2022 still gates <stdatomic.h> behind this switch for C sources.
  target_compile_options(mirctl_core PRIVATE /W4 /utf-8 /experimental:c11atomics)
  target_compile_options(mirctl PRIVATE /W4 /utf-8 /experimental:c11atomics)
  # Windows GUI subsystem prevents a console window when mirctl.exe is opened
  # from Explorer. Keep mainCRTStartup so the same portable main(argc, argv)
  # implementation continues to handle command-line arguments.
  set_target_properties(mirctl PROPERTIES WIN32_EXECUTABLE TRUE)
  set_property(TARGET mirctl_core mirctl PROPERTY
    MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>"
  )
  target_link_options(mirctl PRIVATE
    "/ENTRY:mainCRTStartup"
    "$<$<CONFIG:Release>:/OPT:REF>"
    "$<$<CONFIG:Release>:/OPT:ICF>"
  )
endif()

install(TARGETS mirctl RUNTIME DESTINATION bin)

include(CTest)
if(BUILD_TESTING)
  add_executable(mirctl_protocol_test tests/protocol_test.c src/control.c)
  target_include_directories(mirctl_protocol_test PRIVATE src)
  target_link_libraries(mirctl_protocol_test PRIVATE SDL3::SDL3)
  target_compile_definitions(mirctl_protocol_test PRIVATE
    MC_FIXTURES_PATH="${CMAKE_CURRENT_SOURCE_DIR}/../../protocol/fixtures")
  if(MSVC)
    set_property(TARGET mirctl_protocol_test PROPERTY
      MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
  endif()
  add_test(NAME gsi1_protocol COMMAND mirctl_protocol_test)
endif()
