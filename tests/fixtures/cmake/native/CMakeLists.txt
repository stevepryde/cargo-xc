cmake_minimum_required(VERSION 3.15)
project(cargo_xc_cmake C)
add_library(xc_cmake STATIC answer.c)
install(TARGETS xc_cmake ARCHIVE DESTINATION lib)
